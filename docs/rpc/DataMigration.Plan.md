# DataMigration.Plan

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Plan(PlanMigrationRequest) returns (MigrationPlan);`

- 完整方法：`/xelab.v1.DataMigration/Plan`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：已批准升级/回滚操作。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

source_schema必须匹配当前卷记录，target匹配目标插件版本；Plan不得修改数据。返回计划ID/摘要/临时空间需求，并持久化用于Apply核对。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `deployment_id` | `string` | 按业务校验必填/默认值 |
| 3 | `source_schema` | `string` | 按业务校验必填/默认值 |
| 4 | `target_schema` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `plan_id` | `string` | 按业务校验必填/默认值 |
| 2 | `plan_digest` | `string` | 按业务校验必填/默认值 |
| 3 | `requires_exclusive_write` | `bool` | 按业务校验必填/默认值 |
| 4 | `backward_read_compatible` | `bool` | 按业务校验必填/默认值 |
| 5 | `reversible` | `bool` | 按业务校验必填/默认值 |
| 6 | `temporary_bytes_required` | `uint64` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/migration.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
