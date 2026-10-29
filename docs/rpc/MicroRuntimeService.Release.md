# MicroRuntimeService.Release

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Release(MicroReleaseRequest) returns (Ack);`

- 完整方法：`/xelab.v1.MicroRuntimeService/Release`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：内核已撤路由的微部署控制操作，仅内核mTLS身份。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

按prepared_handle和micro_deployment_id/generation撤销新Invoke并释放缓存引用；不存在或已释放返回accepted=true。不能用旧代Release影响新代句柄，活动调用先按核心Cancel收敛，不把删除缓存视作回滚输出。只影响微绑定，不停止父MPR或删除核心制品。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 3 | `micro_deployment_id` | `string` | 按业务校验必填/默认值 |
| 4 | `micro_generation` | `uint64` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `request_id` | `string` | 按业务校验必填/默认值 |
| 2 | `accepted` | `bool` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/micro.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
