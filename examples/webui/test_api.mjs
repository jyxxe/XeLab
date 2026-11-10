// Component tests use explicit fake fetch/storage. They do not claim a live
// OAuth/core/OCI or browser end-to-end path.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { ApiClient, ApiError, OPERATIONS, encodeRequestJson, readJson, strictJson, validateConfig } from './public/api.js';
import { OAuthSession } from './public/oauth.js';

const config = { core_origin: 'https://kernel.example.test', client_id: 'xelab-0123456789abcdef0123456789abcdef' };
const shell = 'https://desktop.example.test/ui/org.xelab.webui/';
const callback = 'https://desktop.example.test/ui/org.xelab.webui/callback';
const id = '12345678-1234-4234-8234-123456789abc';
const json = (data, status = 200, headers = {}) => new Response(JSON.stringify(data), {
  status, headers: { 'content-type': 'application/json', ...headers },
});
const tokens = (suffix = 'one') => ({ access_token: `access-${suffix}`, token_type: 'Bearer', expires_in: 900,
  refresh_token: `refresh-${suffix}`, scope: `client:${config.client_id}` });
class Storage {
  data = new Map();
  getItem(key) { return this.data.get(key) ?? null; }
  setItem(key, value) { this.data.set(key, String(value)); }
  removeItem(key) { this.data.delete(key); }
}
function flow(fetcher, supplied = {}) {
  const storage = supplied.storage ?? new Storage();
  let navigation;
  const session = new OAuthSession(config, {
    storage, fetcher, getLocation: () => shell, navigate: value => { navigation = value; }, ...supplied,
  });
  return { storage, session, navigation: () => new URL(navigation) };
}
async function signedIn(fetcher = async () => json(tokens()), supplied = {}) {
  const state = flow(fetcher, supplied);
  await state.session.startLogin();
  const auth = state.navigation();
  await state.session.finishLogin(`${callback}?code=one-use-code&state=${auth.searchParams.get('state')}`);
  return state;
}

test('every desktop operation matches the published OpenAPI and excludes password/future APIs', () => {
  const api = JSON.parse(fs.readFileSync(new URL('../../docs/contracts/openapi.json', import.meta.url), 'utf8'));
  for (const [name, operation] of Object.entries(OPERATIONS)) {
    const declared = api.paths[operation.path]?.[operation.method.toLowerCase()];
    assert.equal(declared?.operationId, name);
    assert.equal(Boolean(declared.requestBody?.required), operation.body);
    for (const key of Object.keys(operation.query)) {
      assert.ok(declared.parameters.some(parameter => parameter.in === 'query' && parameter.name === key), `${name} ${key}`);
    }
    assert.equal(operation.revision, (declared.parameters ?? []).some(parameter => parameter.name === 'If-Match' && parameter.required));
  }
  for (const name of ['login', 'changePassword', 'invokeCapability', 'createSkillBatch', 'createWorkflow', 'createMicroPlugin', 'downloadBlob']) {
    assert.equal(Object.hasOwn(OPERATIONS, name), false);
  }
});

test('configuration requires one exact HTTPS origin and public Client ID', () => {
  assert.deepEqual(validateConfig(config), config);
  for (const origin of ['http://kernel.example.test', 'https://kernel.example.test/', 'https://user@kernel.example.test',
    'https://kernel.example.test/path', 'https://kernel.example.test?target=other', 'https://kernel.example.test#x']) {
    assert.throws(() => validateConfig({ ...config, core_origin: origin }), ApiError);
  }
  assert.throws(() => validateConfig({ ...config, client_id: 'client\nHeader: value' }), ApiError);
});

test('fixed API dispatch sends only OAuth Bearer to configured origin, with no cookies or redirects', async () => {
  const requests = [];
  const api = new ApiClient(config, { accessToken: async () => 'opaque-oauth-token' }, {
    fetcher: async (url, options) => { requests.push({ url, options }); return json({ items: [], next_cursor: null }, 200,
      { etag: '"rev-3"', 'x-request-id': id }); },
  });
  const result = await api.invoke('listAccounts', { query: { parent_id: id, limit: 25 } });
  assert.equal(result.etag, '"rev-3"');
  assert.equal(result.requestId, id);
  assert.equal(requests.length, 1);
  const { url, options } = requests[0];
  assert.equal(new URL(url).origin, config.core_origin);
  assert.equal(new URL(url).pathname, '/api/platform/v1/accounts');
  assert.equal(options.credentials, 'omit');
  assert.equal(options.redirect, 'error');
  assert.equal(options.referrerPolicy, 'no-referrer');
  assert.equal(options.headers.get('authorization'), 'Bearer opaque-oauth-token');
  assert.equal(options.headers.get('cookie'), null);
  assert.equal(options.headers.get('x-user-id'), null);
});

test('arbitrary URL, path, query, headers, missing concurrency proof and invalid JSON are rejected before fetch', async () => {
  let fetched = 0;
  const api = new ApiClient(config, { accessToken: async () => 'token' }, { fetcher: async () => { fetched++; return json({}); } });
  const rejects = [
    () => api.invoke('https://evil.example/steal'),
    () => api.invoke('listSkills', { url: 'https://evil.example/' }),
    () => api.invoke('listSkills', { headers: { 'X-User-ID': id } }),
    () => api.invoke('getAccount', { params: { account_id: '../node' } }),
    () => api.invoke('getAccount', { params: { account_id: id, extra: 'ignored' } }),
    () => api.invoke('getUiContribution', { params: { deployment_id: id, contribution_id: '%2e%2e' } }),
    () => api.invoke('listSkills', { query: { url: 'https://evil.example/' } }),
    () => api.invoke('listSkills', { query: { limit: 201 } }),
    () => api.invoke('getAccount', { params: { account_id: id }, body: {} }),
    () => api.invoke('setDeploymentEnabled', { params: { deployment_id: id }, body: { enabled: true }, idempotencyKey: 'same-key' }),
    () => api.invoke('invokeSkill', { params: { skill_id: id }, body: { input: {} } }),
    () => api.invoke('invokeSkill', { params: { skill_id: id }, body: { input: { bad: NaN } }, idempotencyKey: 'same-key' }),
  ];
  for (const run of rejects) await assert.rejects(run, ApiError);
  assert.equal(fetched, 0);
  const cycle = {}; cycle.self = cycle;
  assert.throws(() => strictJson(cycle), ApiError);
  assert.throws(() => strictJson({ code: () => 3 }), ApiError);
  let nested = {}; for (let n = 0; n < 66; n++) nested = { nested };
  assert.throws(() => strictJson(nested), ApiError);
});

test('request JSON byte limit includes UTF-8 and JSON framing', () => {
  const limit = 4 * 1024 * 1024;
  const payloadBytes = limit - Buffer.byteLength(JSON.stringify({ text: '' }), 'utf8');
  const text = '界'.repeat(Math.floor(payloadBytes / 3)) + 'a'.repeat(payloadBytes % 3);
  const encoded = encodeRequestJson({ text });
  assert.equal(Buffer.byteLength(encoded, 'utf8'), limit);
  assert.ok(encoded.length < limit, 'multi-byte characters must not be counted as one byte');
  assert.equal(JSON.parse(encoded).text, text);
  assert.throws(() => encodeRequestJson({ text: text + 'a' }),
    error => error instanceof ApiError && error.code === 'PAYLOAD_TOO_LARGE');
});

test('write dispatch preserves exact ETag/idempotency proof, stable backend errors and never retries', async () => {
  let count = 0;
  const api = new ApiClient(config, { accessToken: async () => 'token' }, { fetcher: async (_url, options) => {
    count++;
    assert.equal(options.method, 'PUT');
    assert.equal(options.headers.get('if-match'), '"rev-7"');
    assert.equal(options.headers.get('idempotency-key'), 'original-intent-key');
    assert.deepEqual(JSON.parse(options.body), { enabled: false });
    return json({ error: { code: 'PERMISSION_DENIED', message: 'Action or resource is not authorized', retryable: false } }, 403,
      { 'x-request-id': id });
  } });
  await assert.rejects(() => api.invoke('setSkillEnabled', { params: { skill_id: id }, body: { enabled: false },
    etag: '"rev-7"', idempotencyKey: 'original-intent-key' }), error => error.code === 'PERMISSION_DENIED'
      && error.status === 403 && error.requestId === id);
  assert.equal(count, 1);
});

test('dispatch notification is not called for local validation or access-token failure', async () => {
  let notifications = 0;
  let fetches = 0;
  let tokenCalls = 0;
  const api = new ApiClient(config, { accessToken: async () => {
    tokenCalls++;
    throw new ApiError('AUTH_REQUIRED', '登录已结束。', 401);
  } }, { fetcher: async () => { fetches++; return json({}); } });
  const onDispatch = () => { notifications++; };
  for (const [name, options] of [
    ['unknownOperation', {}],
    ['invokeSkill', { params: { skill_id: id }, body: { input: { bad: NaN } }, idempotencyKey: 'intent-key' }],
    ['setSkillEnabled', { params: { skill_id: id }, body: { enabled: true }, idempotencyKey: 'intent-key' }],
  ]) {
    await assert.rejects(() => api.invoke(name, options, onDispatch), ApiError);
  }
  assert.equal(tokenCalls, 0);
  await assert.rejects(() => api.invoke('invokeSkill', {
    params: { skill_id: id }, body: { input: {} }, idempotencyKey: 'intent-key',
  }, onDispatch), error => error.code === 'AUTH_REQUIRED');
  assert.equal(tokenCalls, 1);
  assert.equal(notifications, 0);
  assert.equal(fetches, 0);
});

test('dispatch notification runs synchronously immediately before attempted business fetch', async () => {
  const events = [];
  let releaseToken;
  const waitForToken = new Promise(resolve => { releaseToken = resolve; });
  const api = new ApiClient(config, { accessToken: async () => {
    events.push('token-start');
    await waitForToken;
    events.push('token-resolved');
    return 'private-test-access-token';
  } }, { fetcher: async (_url, options) => {
    assert.deepEqual(events, ['token-start', 'token-resolved', 'dispatch']);
    events.push('fetch');
    assert.equal(options.headers.get('idempotency-key'), 'intent-key');
    assert.deepEqual(JSON.parse(options.body), { input: { value: 'private-test-payload' } });
    throw new TypeError('private-test-transport-detail');
  } });
  let callbackArguments;
  const pending = api.invoke('invokeSkill', {
    params: { skill_id: id }, body: { input: { value: 'private-test-payload' } }, idempotencyKey: 'intent-key',
  }, (...args) => { callbackArguments = args; events.push('dispatch'); });
  assert.deepEqual(events, ['token-start']);
  releaseToken();
  await assert.rejects(pending, error => error.code === 'CONNECTION_FAILED'
    && !error.message.includes('private-test-transport-detail'));
  assert.deepEqual(events, ['token-start', 'token-resolved', 'dispatch', 'fetch']);
  assert.deepEqual(callbackArguments, [], 'dispatch callback must not receive token, payload, or request metadata');
});

test('caller cancellation reasons are classified consistently before headers and during the body', async t => {
  const reasons = ['private cancellation detail', new Error('private cancellation detail'),
    new DOMException('private cancellation detail', 'TimeoutError'), null];
  for (const stage of ['headers', 'body']) {
    for (const [index, reason] of reasons.entries()) {
      await t.test(`${stage}, reason ${index}`, async () => {
        const controller = new AbortController();
        let requests = 0;
        const api = new ApiClient(config, { accessToken: async () => 'token' }, {
          fetcher: async (_url, options) => {
            requests++;
            assert.equal(options.headers.get('idempotency-key'), 'original-intent-key');
            if (stage === 'headers') {
              controller.abort(reason);
              throw options.signal.reason;
            }
            return new Response(new ReadableStream({start(stream) {
              stream.enqueue(new TextEncoder().encode('{'));
              queueMicrotask(() => {
                controller.abort(reason);
                stream.error(options.signal.reason);
              });
            }}), {headers: {'content-type': 'application/json'}});
          },
        });
        await assert.rejects(() => api.invoke('invokeSkill', {
          params: {skill_id: id}, body: {input: {}}, idempotencyKey: 'original-intent-key',
          signal: controller.signal,
        }), error => error instanceof ApiError && error.code === 'CANCELED'
          && !error.message.includes('private cancellation detail'));
        assert.equal(requests, 1, 'a canceled write must not be replayed');
      });
    }
  }
});

test('a connection failure without an aborted signal remains a connection failure', async () => {
  const api = new ApiClient(config, { accessToken: async () => 'token' }, {
    fetcher: async () => { throw new TypeError('private transport detail'); },
  });
  await assert.rejects(() => api.invoke('listSkills'), error => error.code === 'CONNECTION_FAILED'
    && !error.message.includes('private transport detail'));
});

test('a revoked OAuth credential invalidates local session state before reporting 401', async () => {
  let loggedOut = false;
  const api = new ApiClient(config, { accessToken: async () => 'revoked-token', logout: () => { loggedOut = true; } }, {
    fetcher: async () => json({ error: { code: 'INVALID_CREDENTIAL', message: 'Invalid credentials' } }, 401),
  });
  await assert.rejects(() => api.invoke('listSkills'), error => error.code === 'INVALID_CREDENTIAL' && error.status === 401);
  assert.equal(loggedOut, true);
});

test('JSON responses are bounded before and during body reading', async () => {
  let canceled = false;
  const response = new Response(new ReadableStream({
    start(controller) { controller.enqueue(new Uint8Array(200)); },
    cancel() { canceled = true; },
  }), { headers: { 'content-type': 'application/json' } });
  await assert.rejects(() => readJson(response, 100), error => error.code === 'PAYLOAD_TOO_LARGE');
  assert.equal(canceled, true);
  await assert.rejects(() => readJson(json({}, 200, { 'content-length': '300' }), 100), error => error.code === 'PAYLOAD_TOO_LARGE');
  await assert.rejects(() => readJson(new Response('<html>not JSON</html>')), error => error.code === 'INVALID_RESPONSE');
});

test('JSON decoding enforces exact byte limits and strict UTF-8 across network chunks', async () => {
  const value = {message: '雪🙂'};
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  const streamed = (payload, cut) => new Response(new ReadableStream({start(controller) {
    controller.enqueue(payload.slice(0, cut));
    controller.enqueue(payload.slice(cut));
    controller.close();
  }}), {headers: {'content-type': 'application/json'}});
  for (let cut = 1; cut < bytes.length; cut++) {
    assert.deepEqual(await readJson(streamed(bytes, cut), bytes.length), value);
  }
  await assert.rejects(() => readJson(streamed(bytes, 1), bytes.length - 1), error => error.code === 'PAYLOAD_TOO_LARGE');
  await assert.rejects(() => readJson(streamed(new Uint8Array([34, 255, 34]), 1)), error => error.code === 'INVALID_RESPONSE');
  await assert.rejects(() => readJson(streamed(new TextEncoder().encode('{'), 1)), error => error.code === 'INVALID_RESPONSE');
  assert.equal(await readJson(new Response(null, {status: 204})), null);
});

test('OAuth navigation uses S256, random state, exact callback and registered Client ceiling only', async () => {
  const state = flow(async () => { throw new Error('No token request expected'); });
  await state.session.startLogin();
  const url = state.navigation();
  assert.equal(url.origin, config.core_origin);
  assert.equal(url.pathname, '/oauth/authorize');
  assert.equal(url.searchParams.get('response_type'), 'code');
  assert.equal(url.searchParams.get('redirect_uri'), callback);
  assert.equal(url.searchParams.get('scope'), `client:${config.client_id}`);
  assert.equal(url.searchParams.get('code_challenge_method'), 'S256');
  assert.match(url.searchParams.get('state'), /^[A-Za-z0-9_-]{43}$/);
  const pending = JSON.parse([...state.storage.data.values()][0]);
  const hash = Buffer.from(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(pending.verifier))).toString('base64url');
  assert.equal(url.searchParams.get('code_challenge'), hash);
  assert.equal(url.searchParams.has('code_verifier'), false);
  assert.equal(url.searchParams.has('client_secret'), false);
  assert.equal(url.searchParams.has('password'), false);
  const firstState = url.searchParams.get('state');
  await state.session.startLogin();
  assert.notEqual(state.navigation().searchParams.get('state'), firstState);
});

test('authorization code exchange is single use and credentials omit; tokens remain in own scoped session storage', async () => {
  const requests = [];
  const state = await signedIn(async (url, options) => { requests.push({ url, options }); return json(tokens()); });
  assert.equal(state.session.loggedIn, true);
  assert.equal(await state.session.accessToken(), 'access-one');
  assert.equal(requests.length, 1);
  const { url, options } = requests[0];
  assert.equal(url, `${config.core_origin}/oauth/token`);
  assert.equal(options.credentials, 'omit');
  assert.equal(options.redirect, 'error');
  assert.equal(options.headers.Authorization, undefined);
  const fields = new URLSearchParams(options.body);
  assert.deepEqual([...fields.keys()].sort(), ['client_id', 'code', 'code_verifier', 'grant_type', 'redirect_uri']);
  assert.equal(fields.get('grant_type'), 'authorization_code');
  assert.equal(fields.get('redirect_uri'), callback);
  assert.equal([...state.storage.data.keys()].length, 1);
  assert.ok([...state.storage.data.keys()][0].includes(`${config.core_origin}:${config.client_id}`));
  const originalState = state.navigation().searchParams.get('state');
  await assert.rejects(() => state.session.finishLogin(`${callback}?code=one-use-code&state=${originalState}`), error => error.code === 'INVALID_STATE');
  assert.equal(requests.length, 1);
});

test('callback rejects redirect/state/query substitutions and never exchanges a code on a mismatch', async () => {
  for (const kind of ['origin', 'path', 'state', 'duplicate', 'unknown', 'fragment']) {
    let calls = 0;
    const state = flow(async () => { calls++; return json(tokens()); });
    await state.session.startLogin();
    const nonce = state.navigation().searchParams.get('state');
    let url = `${callback}?code=private-code&state=${nonce}`;
    if (kind === 'origin') url = url.replace('desktop.example.test', 'evil.example.test');
    if (kind === 'path') url = url.replace('/callback?', '/other?');
    if (kind === 'state') url = url.replace(nonce, 'wrong-state');
    if (kind === 'duplicate') url += '&state=again';
    if (kind === 'unknown') url += '&target=https://evil.example/';
    if (kind === 'fragment') url += '#access_token=bad';
    await assert.rejects(() => state.session.finishLogin(url), ApiError);
    assert.equal(calls, 0, kind);
    assert.equal(state.session.loggedIn, false);
  }
});

test('OAuth denied consent and invalid token schema yield no local login', async () => {
  const denied = flow(async () => { throw new Error('No exchange on denied consent'); });
  await denied.session.startLogin();
  await assert.rejects(() => denied.session.finishLogin(`${callback}?error=access_denied&state=${denied.navigation().searchParams.get('state')}`),
    error => error.code === 'CONSENT_DENIED');
  assert.equal(denied.storage.data.size, 0);
  for (const bad of [{ ...tokens(), token_type: 'Other' }, { ...tokens(), expires_in: 3600 }, { ...tokens(), client_secret: 'not-allowed' }]) {
    const state = flow(async () => json(bad));
    await state.session.startLogin();
    await assert.rejects(() => state.session.finishLogin(`${callback}?code=one-use-code&state=${state.navigation().searchParams.get('state')}`),
      error => error.code === 'INVALID_RESPONSE');
    assert.equal(state.session.loggedIn, false);
    assert.equal(state.storage.data.size, 0);
  }
});

test('access token without optional refresh remains usable until its actual expiry', async () => {
  let calls = 0;
  const state = await signedIn(async () => {
    calls++;
    const data = tokens();
    delete data.refresh_token;
    return json(data);
  });
  const expiresAt = JSON.parse([...state.storage.data.values()][0]).expires_at;
  const originalNow = Date.now;
  try {
    Date.now = () => expiresAt - 1;
    assert.equal(state.session.loggedIn, true);
    assert.equal(await state.session.accessToken(), 'access-one');
    assert.equal(calls, 1, 'no refresh exchange is possible');
    Date.now = () => expiresAt;
    assert.equal(state.session.loggedIn, false);
    await assert.rejects(() => state.session.accessToken(), error => error.code === 'AUTH_REQUIRED');
    assert.equal(state.storage.data.size, 0);
  } finally {
    Date.now = originalNow;
  }
});

test('near-expiry access requests wait for the same rotating refresh', async () => {
  let release;
  let refreshCalls = 0;
  const state = await signedIn(async (_url, options) => {
    if (new URLSearchParams(options.body).get('grant_type') === 'authorization_code') return json(tokens());
    refreshCalls++;
    return new Promise(resolve => { release = () => resolve(json(tokens('two'))); });
  });
  const expiresAt = JSON.parse([...state.storage.data.values()][0]).expires_at;
  const originalNow = Date.now;
  try {
    Date.now = () => expiresAt - 1000;
    const first = state.session.accessToken();
    await new Promise(resolve => setImmediate(resolve));
    const second = state.session.accessToken();
    let secondSettled = false;
    second.then(() => { secondSettled = true; }, () => { secondSettled = true; });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(refreshCalls, 1);
    assert.equal(secondSettled, false);
    release();
    assert.deepEqual(await Promise.all([first, second]), ['access-two', 'access-two']);
  } finally {
    Date.now = originalNow;
  }
});

test('rotating refresh is singleflight and old credential is removed before network; failure clears family locally', async () => {
  let release;
  let refreshCalls = 0;
  let state;
  state = await signedIn(async (_url, options) => {
    const fields = new URLSearchParams(options.body);
    if (fields.get('grant_type') === 'authorization_code') return json(tokens());
    refreshCalls++;
    assert.equal(fields.get('refresh_token'), 'refresh-one');
    assert.equal(state.storage.data.size, 0);
    return new Promise(resolve => { release = () => resolve(json(tokens('two'))); });
  });
  const first = state.session.refresh();
  const second = state.session.refresh();
  assert.equal(first, second);
  assert.equal(refreshCalls, 1);
  release();
  await Promise.all([first, second]);
  assert.equal(await state.session.accessToken(), 'access-two');
  assert.ok([...state.storage.data.values()][0].includes('refresh-two'));
  let sent;
  const failed = await signedIn(async (_url, options) => {
    const fields = new URLSearchParams(options.body);
    if (fields.get('grant_type') === 'authorization_code') return json(tokens());
    sent = fields.get('refresh_token');
    return json({ error: 'invalid_grant', error_description: 'sensitive details not rendered' }, 400);
  });
  await assert.rejects(() => failed.session.refresh(), error => error.code === 'invalid_grant' && !error.message.includes('sensitive'));
  assert.equal(sent, 'refresh-one');
  assert.equal(failed.session.loggedIn, false);
  assert.equal(failed.storage.data.size, 0);
});

test('logout during an in-flight refresh cannot resurrect tokens or login state', async () => {
  let release;
  const state = await signedIn(async (_url, options) => {
    if (new URLSearchParams(options.body).get('grant_type') === 'authorization_code') return json(tokens());
    return new Promise(resolve => { release = () => resolve(json(tokens('late'))); });
  });
  const pending = state.session.refresh();
  state.session.logout();
  release();
  await assert.rejects(() => pending, error => error.code === 'AUTH_REQUIRED');
  assert.equal(state.session.loggedIn, false);
  assert.equal(state.storage.data.size, 0);
});

test('an old in-flight refresh cannot erase a newer explicit login', async () => {
  let release;
  const state = await signedIn(async (_url, options) => {
    const fields = new URLSearchParams(options.body);
    if (fields.get('grant_type') === 'authorization_code') return json(tokens(fields.get('code')));
    return new Promise(resolve => { release = () => resolve(json(tokens('late-old-family'))); });
  });
  const oldRefresh = state.session.refresh();
  await state.session.startLogin();
  await state.session.finishLogin(`${callback}?code=new-user-code&state=${state.navigation().searchParams.get('state')}`);
  release();
  await assert.rejects(() => oldRefresh, error => error.code === 'AUTH_REQUIRED');
  assert.equal(await state.session.accessToken(), 'access-new-user-code');
  assert.equal(state.session.loggedIn, true);
});

test('an expired session stays available while its rotating refresh is pending', async () => {
  let release;
  let refreshCalls = 0;
  const state = await signedIn(async (_url, options) => {
    if (new URLSearchParams(options.body).get('grant_type') === 'authorization_code') return json(tokens());
    refreshCalls++;
    return new Promise(resolve => { release = () => resolve(json(tokens('refreshed'))); });
  });
  const expiresAt = JSON.parse([...state.storage.data.values()][0]).expires_at;
  const originalNow = Date.now;
  let pending;
  try {
    Date.now = () => expiresAt + 1;
    pending = state.session.accessToken();
    assert.equal(state.session.loggedIn, true, 'a valid refresh is still in flight');
    const concurrent = state.session.accessToken();
    assert.equal(refreshCalls, 1);
    release();
    assert.deepEqual(await Promise.all([pending, concurrent]), ['access-refreshed', 'access-refreshed']);
    assert.equal(state.session.loggedIn, true);
  } finally {
    release?.();
    if (pending) await pending.catch(() => {});
    Date.now = originalNow;
  }
});

test('a previous session refresh cannot occupy or clear a new session refresh', async () => {
  const releases = new Map();
  const requested = [];
  const state = await signedIn(async (_url, options) => {
    const fields = new URLSearchParams(options.body);
    if (fields.get('grant_type') === 'authorization_code') return json(tokens(fields.get('code')));
    const token = fields.get('refresh_token');
    requested.push(token);
    return new Promise(resolve => { releases.set(token, () => resolve(json(tokens(`rotated-${token}`)))); });
  });
  const oldRefresh = state.session.refresh();
  const oldResult = oldRefresh.catch(error => error);
  let newRefresh;
  try {
    await state.session.startLogin();
    await state.session.finishLogin(`${callback}?code=new-user-code&state=${state.navigation().searchParams.get('state')}`);
    newRefresh = state.session.refresh();
    assert.notEqual(newRefresh, oldRefresh);
    assert.deepEqual(requested, ['refresh-one-use-code', 'refresh-new-user-code']);
    releases.get('refresh-one-use-code')();
    assert.equal((await oldResult).code, 'AUTH_REQUIRED');
    assert.equal(state.session.refresh(), newRefresh, 'old completion must not clear the new singleflight');
    assert.equal(requested.length, 2);
    releases.get('refresh-new-user-code')();
    await newRefresh;
    assert.equal(await state.session.accessToken(), 'access-rotated-refresh-new-user-code');
  } finally {
    for (const release of releases.values()) release();
    await Promise.allSettled([oldRefresh, ...(newRefresh ? [newRefresh] : [])]);
  }
});

test('a late 401 from an older API request cannot log out a new account session', async () => {
  let release;
  const state = await signedIn(async (_url, options) => json(tokens(new URLSearchParams(options.body).get('code'))));
  const api = new ApiClient(config, state.session, { fetcher: async () => new Promise(resolve => {
    release = () => resolve(json({ error: { code: 'INVALID_CREDENTIAL', message: 'Invalid credentials' } }, 401));
  }) });
  const previousRequest = api.invoke('listSkills');
  await new Promise(resolve => setImmediate(resolve));
  await state.session.startLogin();
  await state.session.finishLogin(`${callback}?code=new-account&state=${state.navigation().searchParams.get('state')}`);
  release();
  await assert.rejects(() => previousRequest, error => error.status === 401);
  assert.equal(await state.session.accessToken(), 'access-new-account');
});

test('different Client or core binding cannot reuse stored local token state', async () => {
  const original = await signedIn();
  const stranger = new OAuthSession({ ...config, client_id: 'xelab-other-client' }, {
    storage: original.storage, fetcher: async () => { throw new Error('No request expected'); }, getLocation: () => shell,
  });
  assert.equal(stranger.loggedIn, false);
  await assert.rejects(() => stranger.accessToken(), error => error.code === 'AUTH_REQUIRED');
  assert.equal(original.session.loggedIn, true);
});
