# 认证、会话与凭证

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 主体与认证方式

User/ServiceAccount 是授权主体；Session/API Key 是凭证；WorkloadIdentity 是实际插件实例身份；Client 只标识 UI 应用。所有派生来源保存在内核，禁止信任 X-User-ID/X-Plugin-ID。

本机可信引导创建首个独立 Owner。待激活子用户使用 24h 单次激活 token 设置密码；服务账户不接受密码登录，使用批准的 API Key 执行后台请求。激活与签发摘要落库后明文不持久化。

密码 12–128 字符，不截断，使用成熟 Argon2id 库并为每个密码生成随机 salt；参数下限采用 OWASP 推荐的内存至少 19 MiB、迭代 2、并行度 1。登录错误统一 INVALID_CREDENTIAL，避免泄漏账户存在性。[密码存储指南](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)

## 会话

可信 login 返回不透明会话令牌，最长 1h；浏览器同时设置内核来源的 `__Host-xelab_auth`，Secure、HttpOnly、SameSite=Lax、Path=/，不设置 Domain。第三方 UI 不调用密码登录，使用受控 OAuth code/PKCE 流。

会话令牌和 API Key 由 CSPRNG 生成至少 256 bit 随机熵；保存 HMAC-SHA256 校验摘要与 key_id，HMAC 密钥存放宿主密钥文件，不能写入数据库或日志。摘要校验采用恒定时间比较。

登出只撤销该会话。账户禁用使该账户及其后代凭证链全部停止授权；恢复账户不复活已 revoked 凭证。

## WebUI 授权

authorize 只接受精确注册 redirect_uri，PKCE 仅 S256。可信登录后由内核批准注册范围的子集并签发授权码；客户端不能自行提交账户 ID 代替会话主体。code 绑定 client/URI/challenge、60s、单次使用。

token 的 code 与 refresh 分支字段定义见 [OAuthTokenRequest](../api/models/OAuthTokenRequest.md)。刷新凭证每次轮换并记录族 ID，重用即吊销同族。敏感 Cookie 操作校验 CSRF token 和 Origin；CLI 无 Origin 时必须显式提供 Bearer 或密码凭证，不依据 localhost 自动放行。

登录按账户与来源分别限制为每分钟 5 次失败，超限 429/Retry-After；成功不抹去审计。MFA/外部 IdP 联邦属于后续认证适配扩展，当前接口未宣称实现。
