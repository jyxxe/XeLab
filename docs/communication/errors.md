# 稳定错误码与重试分类

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 平台错误包装

ErrorEnvelope 必含 error 和服务端 request_id；ErrorDetail 包含 code、message、retryable、violations、retry_after_ms，可附非敏感 resource_id。OAuth token 的协议错误使用 OAuthError，不包平台 error。

| code | HTTP | gRPC | 是否可自动重试 |
|---|---|---|---|
| INVALID_ARGUMENT | 400 | INVALID_ARGUMENT | 否，修改字段 |
| INVALID_CREDENTIAL | 401 | UNAUTHENTICATED | 否 |
| ACCOUNT_DISABLED | 401 | UNAUTHENTICATED | 否 |
| CREDENTIAL_REVOKED | 401 | UNAUTHENTICATED | 否 |
| AUTHORIZATION_CHAIN_INVALID | 401 | UNAUTHENTICATED | 否 |
| PERMISSION_DENIED | 403 | PERMISSION_DENIED | 否 |
| RESOURCE_NOT_FOUND | 404 | NOT_FOUND | 否 |
| STATE_CONFLICT | 409 | FAILED_PRECONDITION | 读取最新状态后判断 |
| CONTRACT_CONFLICT | 409 | FAILED_PRECONDITION | 否 |
| DEPENDENCY_CONFLICT | 409 | FAILED_PRECONDITION | 条件修复后 |
| QUOTA_CONFLICT | 409 | RESOURCE_EXHAUSTED | 否，调整数量/额度 |
| IDEMPOTENCY_CONFLICT | 409 | ALREADY_EXISTS | 否，键不能复用给不同请求 |
| ONE_TIME_RESPONSE_UNAVAILABLE | 409 | FAILED_PRECONDITION | 不可恢复明文，撤销重签 |
| MIGRATION_INCOMPATIBLE | 409 | FAILED_PRECONDITION | 否 |
| OUTCOME_UNKNOWN | 409 | UNKNOWN | 先核实，不重放 |
| ETAG_MISMATCH | 412 | ABORTED | 读最新版本后显式合并 |
| PAYLOAD_TOO_LARGE | 413 | RESOURCE_EXHAUSTED | 缩小请求 |
| UNSUPPORTED_MEDIA_TYPE | 415 | INVALID_ARGUMENT | 修改编码 |
| PRECONDITION_REQUIRED | 428 | FAILED_PRECONDITION | 补前置条件 |
| RATE_LIMITED | 429 | RESOURCE_EXHAUSTED | 仅允许安全调用按 Retry-After |
| RESOURCE_EXHAUSTED | 503 | RESOURCE_EXHAUSTED | 资源释放后，保留原幂等键 |
| NOT_READY / PROVIDER_UNAVAILABLE | 503 | UNAVAILABLE | 仅未提交或可去重操作 |
| DEADLINE_EXCEEDED | 504 | DEADLINE_EXCEEDED | 先查询结果再决定 |
| INTERNAL | 500 | INTERNAL | 不能断言副作用未发生 |

取消在对象状态中表达 canceled；不能为了 HTTP 连接断开返回虚假的“未执行”。gRPC 取消使用 CANCELLED，仍保留持久化结果查询。

## gRPC 错误详情

失败设置标准 gRPC status，同时 trailing metadata `xelab-error-bin` 序列化 common.proto 的 ErrorDetail；客户端不能从 message 文本匹配错误。服务端不暴露 SQL、路径、凭证或第三方内部错误堆栈。

流式响应发送头之后不能修改 HTTP 状态；按对应 StreamEvent / 兼容 SSE 错误帧终止，再关闭连接，日志保留稳定错误码。

## 批次结果查询

RESULT_NOT_READY为409（未完成），RESULT_UNAVAILABLE为409（失败/取消，查看item.error），RESULT_EXPIRED为410（7天保留期已过）。unknown沿用409 OUTCOME_UNKNOWN，不允许通过重试换键自动重放。UI桥本地协议错误见[消息桥](ui-bridge.md)，不混作后端已经执行成功。

CONFIRMED_NOT_EXECUTED仅用于人工证实未执行后保存的项目error.code；项目记failed，后续是否重做由调用方明确决定，不是自动重试指令。

## 微插件错误

MICRO_SOURCE_INVALID（400）表示非法UTF8/摘要或清单；MICRO_SCRIPT_ERROR（400）为语法/运行异常，正文不含私有堆栈；MICRO_OUTPUT_INVALID（400）表示多JSON/不完整JSON/Schema不符；MICRO_LIMIT_EXCEEDED（413或503，按输出大小/资源类别）表示预算耗尽；MICRO_RUNTIME_UNAVAILABLE（503）使既有微部署等待固定父运行时；MICRO_ARTIFACT_EVICTED（409）表示可按原包/代次重新Prepare。wall超时沿用DEADLINE_EXCEEDED，取消在Call状态表达，不返回部分stdout成功。

## 网页搜索及受控HTTP

SEARCH_INPUT_INVALID为400；SEARCH_NO_PROVIDER、SEARCH_PROVIDERS_EXHAUSTED为503；SEARCH_RESPONSE_INVALID为502。它们来自固定微清单business_errors，经host.fail产生业务失败而非成功JSON。UPSTREAM_OUTCOME_UNKNOWN为409且Call/item进入unknown；平台鉴权/来源链/出口策略失败保留原错误，不按provider失败跳过。上游401/403是外部凭证响应，不混同平台401/403；这些响应正文和认证头不公开。
