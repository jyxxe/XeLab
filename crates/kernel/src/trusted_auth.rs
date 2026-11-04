//! Minimal first-party authentication pages. Passwords and the trusted session
//! stay on the core origin; no plugin asset participates in either page.
use crate::error::{Error, Result};
use axum::{body::Body, response::Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use rand::{rngs::OsRng, RngCore};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Page {
    pub html: String,
    pub content_security_policy: String,
}

impl Page {
    pub(crate) fn into_response(self, request: Uuid) -> Result<Response> {
        Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .header("content-security-policy", self.content_security_policy)
            .header("cache-control", "no-store")
            .header("pragma", "no-cache")
            .header("referrer-policy", "no-referrer")
            .header("x-frame-options", "DENY")
            .header("x-content-type-options", "nosniff")
            .header("x-request-id", request.to_string())
            .body(Body::from(self.html))
            .map_err(|_| Error::unavailable("Cannot construct trusted authentication page"))
    }
}

pub(crate) struct Consent<'a> {
    pub authorization_request: Uuid,
    pub idempotency_key: Uuid,
    pub csrf_token: &'a str,
    pub account_name: &'a str,
    pub client_name: &'a str,
    pub plugin_id: &'a str,
    pub redirect_uri: &'a str,
    pub expires_at: DateTime<Utc>,
    pub rules: &'a Value,
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn script_string(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

fn nonce() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn page(title: &str, body: &str, script: &str) -> Page {
    let nonce = nonce();
    let content_security_policy = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; worker-src 'none'"
    );
    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title} · XeLab</title>
<style nonce="{nonce}">
:root{{color-scheme:light dark;font:16px/1.6 system-ui,sans-serif;background:#f2f4f8;color:#152033}}*{{box-sizing:border-box}}body{{margin:0;padding:clamp(16px,5vw,64px)}}main{{max-width:640px;margin:auto;padding:clamp(20px,4vw,40px);background:#fff;border:1px solid #c9d2df;border-radius:16px}}h1{{line-height:1.25}}label{{display:block;margin-top:20px;font-weight:600}}input{{display:block;width:100%;font:inherit;padding:10px;border:1px solid #65738a;border-radius:6px}}button{{font:inherit;cursor:pointer;padding:10px 18px;border:2px solid #285abc;border-radius:6px;background:#285abc;color:#fff;margin:20px 8px 0 0}}button.secondary{{background:transparent;color:inherit}}button:disabled{{cursor:wait;opacity:.65}}:focus-visible{{outline:3px solid #d87800;outline-offset:3px}}[role=alert]{{min-height:1.6em;color:#a32417}}.boundary{{padding:12px;border-left:4px solid #285abc;background:#eef3ff}}dt{{font-weight:600;margin-top:12px}}dd{{margin:0;overflow-wrap:anywhere}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;font-size:.85rem}}.actions{{display:flex;flex-wrap:wrap}}@media(prefers-color-scheme:dark){{:root{{background:#111722;color:#e9edf5}}main{{background:#1a2434;border-color:#53617a}}.boundary{{background:#223454}}[role=alert]{{color:#ffb5a9}}}}@media(prefers-reduced-motion:reduce){{*{{scroll-behavior:auto}}}}
</style></head><body><main>{body}</main><script nonce="{nonce}">{script}</script></body></html>"#,
        title = escape(title),
    );
    Page {
        html,
        content_security_policy,
    }
}

/// `resume` is built from a validated server-side authorization intent. This
/// extra path check prevents a later caller from introducing a generic next URL.
pub(crate) fn login(resume: Option<&str>) -> Result<Page> {
    let resume = resume.unwrap_or("/");
    if resume != "/"
        && resume != "/auth/ui/login"
        && (!resume.starts_with("/oauth/authorize?")
            || resume.contains(['\r', '\n', '#'])
            || resume.len() > 8192)
    {
        return Err(Error::invalid("Invalid trusted login continuation"));
    }
    let body = r#"<h1>登录 XeLab</h1>
<p class="boundary">这是内核的可信登录页面。密码只提交给当前内核，不会交给 WebUI 或业务插件。</p>
<form id="login" method="post" action="/api/platform/v1/auth/login">
<label for="account">账户 ID</label><input id="account" name="account_id" autocomplete="username" required pattern="[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" spellcheck="false" autocapitalize="none">
<label for="password">密码</label><input id="password" name="password" type="password" autocomplete="current-password" required>
<p id="error" role="alert" aria-live="polite"></p><button id="submit" type="submit">登录</button>
</form><noscript><p>可信登录需要 JavaScript。也可继续使用 xelabctl 登录和管理核心。</p></noscript>"#;
    let script = format!(
        r#"'use strict';
const form = document.getElementById('login');
const error = document.getElementById('error');
const button = document.getElementById('submit');
const resume = {};
form.addEventListener('submit', async event => {{
 event.preventDefault();
 if (button.disabled || !form.reportValidity()) return;
 error.textContent = '';
 button.disabled = true;
 form.setAttribute('aria-busy', 'true');
 try {{
  const response = await fetch('/api/platform/v1/auth/login', {{
   method: 'POST', credentials: 'same-origin', redirect: 'error',
   headers: {{'Content-Type': 'application/json', 'Accept': 'application/json'}},
   body: JSON.stringify({{account_id: document.getElementById('account').value, password: document.getElementById('password').value}})
  }});
  document.getElementById('password').value = '';
  if (!response.ok) {{
   error.textContent = response.status === 429 ? '尝试次数过多，请稍后重试。' : '登录失败，请检查凭证或稍后重试。';
   document.getElementById('password').focus();
   return;
  }}
  // The HTTP-only Cookie is set by the core. Do not copy the access token into
  // the page, storage, a plugin URL, or a postMessage channel.
  window.location.replace(resume);
 }} catch (_) {{
  error.textContent = '无法连接内核，请检查连接后重试。';
 }} finally {{
  button.disabled = false;
  form.removeAttribute('aria-busy');
 }}
}});"#,
        script_string(resume)?,
    );
    Ok(page("可信登录", body, &script))
}

/// Native form submission is intentional: a JSON fetch cannot expose a manual
/// 302 Location to JavaScript or navigate the browser to an OAuth callback.
/// The transport consumes the two existing header-named metadata form fields
/// before validating the unchanged ConsentDecision JSON model.
pub(crate) fn consent(view: Consent<'_>) -> Result<Page> {
    if !view.rules.is_array() || view.csrf_token.is_empty() || view.csrf_token.len() > 512 {
        return Err(Error::invalid("Invalid trusted consent view"));
    }
    let redirect = url::Url::parse(view.redirect_uri)
        .map_err(|_| Error::invalid("Invalid consent redirect URI"))?;
    if redirect.scheme() != "https"
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.fragment().is_some()
        || redirect.host_str().is_none()
    {
        return Err(Error::invalid("Invalid consent redirect URI"));
    }
    let rules = serde_json::to_string_pretty(view.rules)?;
    let body = format!(
        r#"<h1>授权 WebUI</h1>
<p class="boundary">请确认客户端、当前账户和权限范围。插件不会取得密码或内核登录 Cookie。</p>
<dl><dt>当前账户</dt><dd>{account}</dd><dt>客户端</dt><dd>{client}</dd><dt>插件</dt><dd>{plugin}</dd><dt>固定回调地址</dt><dd>{redirect}</dd><dt>本次请求有效期至</dt><dd>{expires}</dd></dl>
<details open><summary>将授予的权限范围</summary><pre>{rules}</pre></details>
<p>授权始终受当前账户、祖先和来源权限限制。拒绝会返回客户端并报告拒绝授权。</p>
<form id="consent" method="post" action="/api/platform/v1/auth/consents" enctype="application/x-www-form-urlencoded">
<input type="hidden" name="authorization_request_id" value="{request}">
<input type="hidden" name="X-CSRF-Token" value="{csrf}">
<input type="hidden" name="Idempotency-Key" value="{key}">
<div class="actions"><button type="submit" name="approved" value="true">允许这些权限</button><button class="secondary" type="submit" name="approved" value="false">拒绝</button></div>
</form>"#,
        account = escape(view.account_name),
        client = escape(view.client_name),
        plugin = escape(view.plugin_id),
        redirect = escape(view.redirect_uri),
        expires = escape(&view.expires_at.to_rfc3339()),
        rules = escape(&rules),
        request = view.authorization_request,
        csrf = escape(view.csrf_token),
        key = view.idempotency_key,
    );
    Ok(page("可信授权", &body, "'use strict';"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_continuation_is_internal_and_script_safe() {
        for value in [
            "https://evil.example/",
            "//evil.example/",
            "/auth/ui/login?next=https://evil.example/",
            "/oauth/authorize?state=ok\r\nLocation: https://evil.example/",
            "/oauth/authorize?state=ok#fragment",
        ] {
            assert!(login(Some(value)).is_err());
        }
        let rendered = login(Some(
            "/oauth/authorize?state=</script><script>evil()</script>",
        ))
        .unwrap();
        assert!(!rendered.html.contains("state=</script>"));
        assert!(rendered.html.contains("\\u003c/script\\u003e"));
        assert!(rendered
            .content_security_policy
            .contains("frame-ancestors 'none'"));
        assert!(!rendered.content_security_policy.contains("unsafe-inline"));
        assert_ne!(
            rendered.content_security_policy,
            login(None).unwrap().content_security_policy
        );
        let response = rendered.into_response(Uuid::new_v4()).unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert!(!response.headers().contains_key("set-cookie"));
    }
    #[test]
    fn consent_escapes_untrusted_client_text_and_uses_native_post() {
        let rules = serde_json::json!([{"action":"plugin.read","resource":{"kind":"plugin","match":"all"}}]);
        let rendered = consent(Consent {
            authorization_request: Uuid::new_v4(),
            idempotency_key: Uuid::new_v4(),
            csrf_token: "a\"&<token>",
            account_name: "Owner",
            client_name: "<script>password()</script>",
            plugin_id: "org.xelab.webui",
            redirect_uri: "https://ui.example/callback?one=1&two=2",
            expires_at: Utc::now(),
            rules: &rules,
        })
        .unwrap();
        assert!(!rendered.html.contains("<script>password()"));
        assert!(rendered.html.contains("&lt;script&gt;password()"));
        assert!(rendered.html.contains("one=1&amp;two=2"));
        assert!(rendered
            .html
            .contains("value=\"a&quot;&amp;&lt;token&gt;\""));
        assert!(rendered.html.contains("name=\"Idempotency-Key\""));
        assert!(rendered.html.contains("name=\"X-CSRF-Token\""));
        assert!(rendered.html.contains("application/x-www-form-urlencoded"));
        assert!(!rendered.html.contains("fetch("));
        assert!(!rendered.html.contains("postMessage"));
    }
}
