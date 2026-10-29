# auth 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| POST | `/api/platform/v1/auth/activate` | [激活子账户](activateAccount.md) |
| POST | `/api/platform/v1/auth/login` | [建立可信登录会话](login.md) |
| GET | `/api/platform/v1/sessions` | [列出本人会话](listSessions.md) |
| DELETE | `/api/platform/v1/sessions/{session_id}` | [吊销会话](revokeSession.md) |
| POST | `/oauth/token` | [交换授权码或轮换刷新凭证](exchangeOAuthToken.md) |
| GET | `/oauth/authorize` | [浏览器授权与回调](authorizeOAuth.md) |
| GET | `/auth/ui/login` | [内核最小可信登录页](getTrustedLoginPage.md) |
| GET | `/auth/ui/consent` | [内核最小可信授权页](getTrustedConsentPage.md) |
| POST | `/api/platform/v1/auth/consents` | [批准或拒绝浏览器委托](submitConsent.md) |
