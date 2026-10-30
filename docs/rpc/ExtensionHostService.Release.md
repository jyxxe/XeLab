# ExtensionHostService.Release

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Release(ExtensionReleaseRequest) returns (Ack);`

- 完整方法：`/xelab.v1.ExtensionHostService/Release`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：核心已确认的扩展停用/归档/宿主失效清理；只清理精确旧绑定，不要求被撤销的业务授权继续有效。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

核对extension deployment/generation、host_chain_digest与root实例/代次；先撤新路由，取消该扩展及依赖后代queued/在途和固定handler子Call，逆拓扑卸载贡献及准备句柄。对象或句柄已不存在可幂等Ack，不删除版本/Call/业务数据。旧代清理不得释放新代绑定，也不能卸载兄弟扩展；清理/传输超时不是业务未执行或已排空证明。等待micro worker/handler执行真实结束及资源租约回收后才收敛disabled/archived；root离线时持久待清理记录并waiting_host，不自动改绑其它宿主。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 3 | `extension_deployment_id` | `string` | 按业务校验必填/默认值 |
| 4 | `extension_generation` | `uint64` | 按业务校验必填/默认值 |
| 5 | `host_chain_digest` | `string` | 按业务校验必填/默认值 |
| 6 | `root_instance_id` | `string` | 按业务校验必填/默认值 |
| 7 | `root_generation` | `uint64` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `request_id` | `string` | 按业务校验必填/默认值 |
| 2 | `accepted` | `bool` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/extension.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
