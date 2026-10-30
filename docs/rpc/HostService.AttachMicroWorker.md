# HostService.AttachMicroWorker

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc AttachMicroWorker(AttachMicroWorkerRequest) returns (Ack);`

- 完整方法：`/xelab.v1.HostService/AttachMicroWorker`
- 允许对端：`plugin`；工作负载身份从通道验证，禁止正文自报。
- 授权：当前micro目标或已授权micro物理执行根ExtensionCall的MPR监督器委托 ∩ micro.invoke ∩ micro.runtime.use；ExtensionCall另查leaf extension.invoke与每层extension.host.use，不新增用户宿主资源管理权。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

仅当前Call真实MPR实例/代次可请求，resource_group_handle须由核心为该Call/物理执行/微部署代次签发；所有UUID/代次/容器PID和启动时间必须匹配。核心Runtime.attach_micro_worker映射PID namespace、核对同实例/监督器子进程及启动时间、取pidfd防复用，将阻塞且未装载用户代码的worker迁入真实微cgroup，复核成功才Ack accepted=true。MPR收到Ack之前不得运行用户代码。失败kill/reap待启动worker并回收，禁止附宿主路径或直接指定host PID，MPR/VM不获得cgroup写权。脚本没有此Host入口；attach不是HTTP服务商请求，不计sequence1..3。 MPR1.3允许target.kind=extension的Call，但仅当核心已验证其ExtensionIdentityClosure最终逻辑宿主为该micro版本/部署，且Call.micro_*快照、root实例/代次、资源租约均一致；不能仅凭请求自报微ID或伪造source_kind取得Attach。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `call_id` | `string` | 按业务校验必填/默认值 |
| 3 | `resource_group_handle` | `string` | 按业务校验必填/默认值 |
| 4 | `container_pid` | `uint32` | 按业务校验必填/默认值 |
| 5 | `process_start_time_ticks` | `uint64` | 按业务校验必填/默认值 |
| 6 | `micro_deployment_id` | `string` | 按业务校验必填/默认值 |
| 7 | `micro_deployment_generation` | `uint64` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `request_id` | `string` | 按业务校验必填/默认值 |
| 2 | `accepted` | `bool` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/host.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
