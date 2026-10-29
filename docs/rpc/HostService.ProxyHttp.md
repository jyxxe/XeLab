# HostService.ProxyHttp

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc ProxyHttp(MicroProxyHttpRequest) returns (MicroProxyHttpResponse);`

- 完整方法：`/xelab.v1.HostService/ProxyHttp`
- 允许对端：`plugin`；工作负载身份从通道验证，禁止正文自报。
- 授权：micro.http.request ∩ micro.invoke ∩ micro.runtime.use ∩ 当前MPR代理审批/出口白名单/受控Secret绑定。
- 期限上限：4000ms；仍取调用整体 deadline 的更小值。
- 幂等：不保证方法无副作用，按业务契约和稳定幂等键判断，禁止 SDK 默认自动重放。

## 前置条件、执行及失败语义

仅当前controlled-http-v1微Call的实际MPR实例和RpcDelegation可调用；call_id须与委托绑定，alias从不可变endpoint及Call冻结HttpBinding快照解析。seq1..3，每alias最多一个真实请求；同(call,seq)同规范指纹重放有界已知结果，不再出网，异内容冲突。发送前持久记录，发送后无确定响应为unknown且封锁Call后续HTTP，最终Call状态不能被脚本stdout或catch覆盖。HTTPS443仅固定GET扁平标量query或POST JSON，无重定向，DNS/连接IP均拒绝私网/回环/link-local；密钥由核心按已批准精确绑定注入，MPR不获原值/LeaseSecret权。请求<=64KiB，解压响应<=512KiB，单次<=4s且取根剩余；节点最多64活动/128排队，队列最多1s仍计根deadline。MicroProxyResult无响应headers，非2xx丢弃原始错误body；2xx若JSON任何字符串含注入Secret原值则拒绝。平台鉴权/输入/出口策略错误终止Call，不伪装provider可回退错误。仅not_sent或已确定HTTP结果可由脚本按同一业务计划选择下一provider，禁止SDK/Core自动重发。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `call_id` | `string` | 按业务校验必填/默认值 |
| 3 | `endpoint_alias` | `string` | 按业务校验必填/默认值 |
| 4 | `sequence` | `uint32` | 按业务校验必填/默认值 |
| 5 | `payload_json` | `bytes` | 按业务校验必填/默认值 |
| 6 | `payload_digest` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `result_json` | `bytes` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/host.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
