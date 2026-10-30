# HTTP 公共契约

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 路径、类型和字段

管理路径 `/api/platform/v1/`；兼容数据面 `/v1/`；内核 OAuth `/oauth/`；可信认证页 `/auth/ui/`；探针 `/health/`。大小写敏感，路径 ID 不接受编码后斜杠、点段或双重解码。

除明确声明的二进制/表单/SSE/HTML 外使用 UTF-8 application/json。对象未知字段和未知查询参数返回 400，重复 JSON key 拒绝；可选字段缺失与显式 null 不等价。时间为 RFC3339 UTC，出接口统一 Z。整数范围以 Schema 为准，最大 2^53-1。

`required` 决定字段是否可省略；default 不会取消 required。必填配置字段的 default 表示建议初始值；可选字段省略时才由服务端应用其声明默认值。响应中的 readOnly 字段由服务端生成，writeOnly 输入不出现在读取投影。

同步管理请求最大 1 MiB、10s；通用 Invoke 与 AI 业务 JSON 最大 4 MiB、单次结果最大 16 MiB；Blob 最大 64 MiB；更低的契约/实例限额优先。超限 413，媒体类型不支持 415。

异步Skill batch提交属于业务入口，请求最大4MiB，10s内完成校验/持久化并返回202，不等待执行；状态/取消/偏好等管理请求仍按1MiB/10s。iframe消息桥另有1MiB请求限制，不能因核心允许4MiB而越过页面通道上限。

## 请求和响应头

| Header | 方向与约束 |
|---|---|
| Authorization | `Bearer <opaque token>`；TLS；禁止日志记录；与 Cookie 同时出现且主体冲突返回 400 |
| X-Request-ID | 响应必需，服务端生成 UUID；客户端提供值仅作外部关联，不覆盖服务端追踪 ID |
| If-Match | 规范指定写接口必需；强 ETag `"rev-N"`；缺失 428，不匹配 412 |
| Idempotency-Key | 规范指定变更接口必需，长度 1–128；去重规则见 idempotency.md |
| ETag | 单对象读取返回当前 revision；列表对象 revision 可用于构造目标 ETag |
| Retry-After | 429/可恢复 503 可返回秒数；不是安全重试授权 |
| X-CSRF-Token | 使用内核 Cookie 发起敏感操作时必需，绑定会话；Bearer-only CLI 不要求 |
| Cache-Control | 凭证、账户私有结果、能力结果均 `no-store` |

权限、实例身份与源链只能由内核建立。转发删除外部 X-User-ID/X-Plugin-ID 等伪造身份头，以受保护委托代替原始用户令牌。

## 并发、分页、错误

写入授权、版本检查与状态变更须在同一事务。资源不可见和不存在均返回 404；无法认证 401，明确越权 403。

列表 limit 默认 50，范围 1–200；cursor 为签名不透明值，绑定主体、凭证链、过滤条件、授权版本及创建时间水位，15min 失效。持久资源按 `(created_at,id)` 升序；日志按 `(at,后端稳定偏移)`；角色/权限等静态目录按名称；能力提供者按 `(contract.name,contract.version,instance_id)`。新创建对象不进入既有水位。既有对象状态变化会改变过滤结果，列表不提供跨请求事务快照保证。游标无效或到期返回 400 INVALID_ARGUMENT，客户端从第一页重新读取。

OpenAPI 返回 schema 精确指定包装；普通对象直接返回，列表 items/next_cursor；`/v1/models` 使用其明确子集包装。204 不返回 JSON。同步成功不等同任务完成，202 必须返回持久化 Operation。

HTTP 条件请求语义参考 [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html)；本平台的额外约束以本规格为准。
