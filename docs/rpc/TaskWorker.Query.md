# TaskWorker.Query

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Query(QueryTaskRequest) returns (TaskResult);`

- 完整方法：`/xelab.v1.TaskWorker/Query`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：task.read + 原执行范围。
- 期限上限：2000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

返回跨重启持久化结果；TaskResult.SUCCEEDED须output_json，FAILED须error，其余状态不携带终态outcome。未知结果不得伪装 FAILED 后让平台重试。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `execution_id` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `execution_id` | `string` | 按业务校验必填/默认值 |
| 2 | `fence` | `uint64` | 按业务校验必填/默认值 |
| 3 | `state` | `ExecutionState` | 按业务校验必填/默认值 |
| 4 | `output_json` | `bytes` | oneof outcome |
| 5 | `error` | `ErrorDetail` | oneof outcome |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/task.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
