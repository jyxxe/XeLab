# MicroRuntimeService.Prepare

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Prepare(MicroPrepareRequest) returns (MicroPrepareResponse);`

- 完整方法：`/xelab.v1.MicroRuntimeService/Prepare`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：内核已授权micro.deploy/enable及micro.runtime.use；运行时仅接受内核mTLS控制。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

验证所有UUID/微代次/包摘要/源码摘要、UTF8和MicroManifest Schema，limits不大于不可变版本及父预算。按micro_deployment_id/generation/package_digest幂等准备；生成绑定当前runtime实例/代次和微代次的prepared_handle，可LRU淘汰但不是授权票据。不得执行用户副作用；源码解析或沙箱自检失败不Ready。整个请求<=1MiB，控制调用无终端用户凭证转发。 MPR1.2要求resources_json为MicroResourceLimits且network_enabled与当前核心微部署设置一致；复用既有micro_generation表示当前MicroDeployment.generation，必填非0，prepared_handle绑定该代次和设置。pure-json-v1不得network=true；network=false不阻止Prepare/Ready。不创建微执行资源组或执行用户代码，后续Invoke按活跃调用准入。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `micro_plugin_id` | `string` | 按业务校验必填/默认值 |
| 3 | `micro_version_id` | `string` | 按业务校验必填/默认值 |
| 4 | `micro_deployment_id` | `string` | 按业务校验必填/默认值 |
| 5 | `micro_generation` | `uint64` | 按业务校验必填/默认值 |
| 6 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 7 | `source_digest` | `string` | 按业务校验必填/默认值 |
| 8 | `manifest_json` | `bytes` | 按业务校验必填/默认值 |
| 9 | `source_utf8` | `bytes` | 按业务校验必填/默认值 |
| 10 | `limits_json` | `bytes` | 按业务校验必填/默认值 |
| 11 | `owner_id` | `string` | 按业务校验必填/默认值 |
| 12 | `resources_json` | `bytes` | 按业务校验必填/默认值 |
| 13 | `network_enabled` | `bool` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 2 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 3 | `observed_micro_generation` | `uint64` | 按业务校验必填/默认值 |
| 4 | `expires_unix_ms` | `int64` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/micro.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
