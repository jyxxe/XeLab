import { ApiError, readJson, validateConfig } from './api.js';

const CALLBACK = '/ui/org.xelab.webui/callback';
const b64url = bytes => btoa(String.fromCharCode(...bytes)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
const random = () => b64url(crypto.getRandomValues(new Uint8Array(32)));
const plain = value => value !== null && typeof value === 'object' && !Array.isArray(value)
  && Object.getPrototypeOf(value) === Object.prototype;
const transientAge = 10 * 60 * 1000;

export class OAuthSession extends EventTarget {
  #config;
  #storage;
  #fetch;
  #navigate;
  #location;
  #key;
  #tokens = null;
  #refreshing = null;
  #epoch = 0;

  constructor(config, {
    storage = globalThis.sessionStorage,
    fetcher = globalThis.fetch,
    navigate = url => globalThis.location.assign(url),
    getLocation = () => globalThis.location.href,
  } = {}) {
    super();
    this.#config = validateConfig(config);
    this.#storage = storage;
    // Preserve the native browser fetch receiver for code/refresh exchanges.
    this.#fetch = fetcher.bind(globalThis);
    this.#navigate = navigate;
    this.#location = getLocation;
    this.#key = `xelab.oauth:${this.#config.core_origin}:${this.#config.client_id}`;
    this.#restore();
  }

  #callback() {
    let current;
    try { current = new URL(this.#location()); } catch { /* checked below */ }
    if (!current || current.protocol !== 'https:' || current.username || current.password
        || current.origin === this.#config.core_origin) {
      throw new ApiError('INVALID_CONFIGURATION', 'WebUI 必须使用独立 HTTPS 来源。');
    }
    return new URL(CALLBACK, current.origin).href;
  }

  #restore() {
    try {
      const data = JSON.parse(this.#storage.getItem(this.#key));
      if (plain(data) && data.core_origin === this.#config.core_origin
          && data.client_id === this.#config.client_id && validToken(data.access_token, 4096)
          && (data.refresh_token === null || validToken(data.refresh_token, 512))
          && Number.isSafeInteger(data.expires_at) && data.expires_at > 0
          && typeof data.scope === 'string' && /^[\x21-\x7e ]{1,512}$/.test(data.scope)) this.#tokens = data;
      else this.#storage.removeItem(this.#key);
    } catch { this.#storage.removeItem(this.#key); }
  }

  get loggedIn() {
    return this.#tokens !== null && (this.#tokens.expires_at > Date.now() || this.#tokens.refresh_token !== null);
  }

  async accessToken() {
    if (!this.#tokens) throw new ApiError('AUTH_REQUIRED', '请通过可信核心登录。', 401);
    if (this.#tokens.expires_at <= Date.now() + 30000) await this.refresh();
    if (!this.#tokens) throw new ApiError('AUTH_REQUIRED', '登录已结束。', 401);
    return this.#tokens.access_token;
  }

  invalidateAccessToken(token) {
    // A rejected old request cannot log out a newer account/client session.
    if (this.#tokens?.access_token === token) this.logout();
  }

  async startLogin() {
    const redirect = this.#callback();
    this.logout();
    const verifier = random();
    const state = random();
    const challenge = b64url(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier))));
    this.#storage.setItem(`${this.#key}:pending`, JSON.stringify({
      core_origin: this.#config.core_origin, client_id: this.#config.client_id,
      redirect_uri: redirect, verifier, state, created_at: Date.now(),
    }));
    const authorize = new URL('/oauth/authorize', this.#config.core_origin);
    authorize.search = new URLSearchParams({
      response_type: 'code', client_id: this.#config.client_id, redirect_uri: redirect,
      state, code_challenge: challenge, code_challenge_method: 'S256', scope: `client:${this.#config.client_id}`,
    }).toString();
    this.#navigate(authorize.href);
  }

  async finishLogin(locationURL) {
    const expected = this.#callback();
    let location;
    try { location = new URL(locationURL); } catch { throw new ApiError('INVALID_CALLBACK', '无效的授权回调。'); }
    if (`${location.origin}${location.pathname}` !== expected || location.hash || location.username || location.password) {
      throw new ApiError('INVALID_CALLBACK', '授权回调地址不匹配。');
    }
    const fields = [...location.searchParams.keys()];
    if (fields.some(key => !['code', 'state', 'error', 'error_description'].includes(key))
        || new Set(fields).size !== fields.length || !location.searchParams.has('state')
        || location.searchParams.has('code') === location.searchParams.has('error')) {
      throw new ApiError('INVALID_CALLBACK', '授权回调字段不匹配。');
    }
    let pending;
    try { pending = JSON.parse(this.#storage.getItem(`${this.#key}:pending`)); } catch { /* checked below */ }
    // Consume before network I/O: a callback or transient verifier is single use.
    this.#storage.removeItem(`${this.#key}:pending`);
    if (!plain(pending) || pending.core_origin !== this.#config.core_origin
        || pending.client_id !== this.#config.client_id || pending.redirect_uri !== expected
        || pending.state !== location.searchParams.get('state') || !/^[A-Za-z0-9_-]{43}$/.test(pending.verifier)
        || !Number.isSafeInteger(pending.created_at) || Date.now() - pending.created_at < 0
        || Date.now() - pending.created_at > transientAge) {
      throw new ApiError('INVALID_STATE', '授权来源或状态已失效，请重新登录。');
    }
    if (location.searchParams.has('error')) throw new ApiError('CONSENT_DENIED', '核心未批准本次授权。', 403);
    const code = location.searchParams.get('code');
    if (!validToken(code, 512) || location.searchParams.has('error_description')) {
      throw new ApiError('INVALID_CALLBACK', '无效的授权码回调。');
    }
    const epoch = this.#epoch;
    const data = await this.#exchange({
      grant_type: 'authorization_code', client_id: this.#config.client_id,
      code, redirect_uri: pending.redirect_uri, code_verifier: pending.verifier,
    });
    if (epoch !== this.#epoch) throw new ApiError('AUTH_REQUIRED', '登录交换期间会话已结束。', 401);
    this.#save(data);
    this.dispatchEvent(new Event('login'));
    return true;
  }

  async #exchange(fields) {
    let response;
    try {
      response = await this.#fetch(new URL('/oauth/token', this.#config.core_origin).href, {
        method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
        body: new URLSearchParams(fields).toString(), credentials: 'omit', mode: 'cors',
        redirect: 'error', cache: 'no-store', referrerPolicy: 'no-referrer',
        signal: AbortSignal.timeout(10000),
      });
    } catch { throw new ApiError('CONNECTION_FAILED', '无法完成授权交换，请重新通过核心登录。'); }
    const data = await readJson(response, 16384);
    if (!response.ok) {
      // Never echo code, verifier, refresh token or server-provided diagnostics.
      const code = typeof data?.error === 'string' && /^[a-z_]{1,40}$/.test(data.error) ? data.error : 'oauth_error';
      throw new ApiError(code, '核心拒绝授权凭证，请重新登录。', response.status);
    }
    if (!plain(data) || Object.keys(data).some(key => !['access_token', 'token_type', 'expires_in', 'refresh_token', 'scope'].includes(key))
        || !validToken(data.access_token, 4096) || data.token_type !== 'Bearer' || data.expires_in !== 900
        || (data.refresh_token !== undefined && !validToken(data.refresh_token, 512))
        || typeof data.scope !== 'string' || !/^[\x21-\x7e ]{1,512}$/.test(data.scope)) {
      throw new ApiError('INVALID_RESPONSE', '核心令牌响应不符合契约。');
    }
    return data;
  }

  #save(data) {
    this.#tokens = {
      core_origin: this.#config.core_origin, client_id: this.#config.client_id,
      access_token: data.access_token, refresh_token: data.refresh_token ?? null,
      expires_at: Date.now() + data.expires_in * 1000, scope: data.scope,
    };
    this.#storage.setItem(this.#key, JSON.stringify(this.#tokens));
  }

  refresh() {
    if (this.#refreshing) return this.#refreshing;
    const token = this.#tokens?.refresh_token;
    if (!token) {
      this.logout();
      return Promise.reject(new ApiError('AUTH_REQUIRED', '登录已过期，请重新登录。', 401));
    }
    // Remove the old rotating credential before sending it. Failed or unknown
    // exchanges require login; no stale tab-local token is retried blindly.
    this.#tokens.refresh_token = null;
    this.#storage.removeItem(this.#key);
    const epoch = this.#epoch;
    this.#refreshing = this.#exchange({ grant_type: 'refresh_token', client_id: this.#config.client_id, refresh_token: token })
      .then(data => {
        if (epoch !== this.#epoch) throw new ApiError('AUTH_REQUIRED', '刷新期间会话已结束。', 401);
        this.#save(data);
      })
      .catch(error => {
        // A late old-family response must not erase a newer explicit login.
        if (epoch === this.#epoch) this.logout();
        throw error;
      })
      .finally(() => { this.#refreshing = null; });
    return this.#refreshing;
  }

  logout() {
    this.#epoch += 1;
    this.#tokens = null;
    this.#storage.removeItem(this.#key);
    this.#storage.removeItem(`${this.#key}:pending`);
    this.dispatchEvent(new Event('logout'));
  }
}

function validToken(value, maximum) {
  return typeof value === 'string' && value.length >= 1 && value.length <= maximum && /^[\x21-\x7e]+$/.test(value);
}
