# MicroRuntimeService.Invoke

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Invoke(MicroInvokeRequest) returns (MicroInvokeResponse);`

- 完整方法：`/xelab.v1.MicroRuntimeService/Invoke`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：skill.invoke ∩ micro.invoke(micro_plugin) ∩ micro.runtime.use(父deployment) ∩ 当前源链/运行时审批。
- 期限上限：30000ms；仍取调用整体 deadline 的更小值。
- 幂等：不保证方法无副作用，按业务契约和稳定幂等键判断，禁止 SDK 默认自动重放。

## 前置条件、执行及失败语义

核心先创建持久Call并锁定Skill/微版本/包/父安装，当前委托须绑定实际MPR实例。核对handle及全部固定ID/代次/摘要、input_digest/Schema和有效deadline，父槽与子槽均准入才创建新受限VM。虚拟stdin一个UTF8 JSON，stdout仅单个完整JSON且正常完成才成功，stdin/stdout各<=1MiB；stderr有界截断。脚本无HostService/文件/网络/secret和运行时凭证。side_effect=none仅表示纯计算可按统一预算安全重算，不承诺崩溃后结果原样缓存；InvokeResponse实际contract由微Skill固定。超时/取消必须销毁VM/worker，不返回部分输出。Call/Cancel/QueryResult沿用核心调用协议，未实现流式。 已识别Call的脚本异常/非法输出/资源失败通过MicroInvokeResponse.outcome.error返回并可附有界诊断，核心记录failed并映射对应HTTP错误，不把gRPC OK当业务成功。result/error恰有一个；未认证、非法控制封装或传输错误使用标准gRPC失败且不泄露日志。超时或进程崩溃可能无法取得诊断，核心仍按Call状态核实。 execution_profile默认pure-json-v1仍按none恢复；controlled-http-v1必须non_repeatable，VM仅暴露host.httpBindings同步投影与host.httpRequest Promise桥，由监督器调用ProxyHttp。只运行受控Promise事件泵，一Call最多1个在途proxy；等待计总wall且不忙等CPU，不开放timer/fetch。主Promise fulfilled且无pendingHostCall才判成功。host.fail只报告MicroManifest.business_errors声明的业务码；出网unknown或平台终止状态由核心优先裁决，不能用正常stdout覆盖。 MPR1.2检查既有micro_generation与Prepare及当前核心Call冻结的MicroDeployment.generation一致。controlled-http-v1且network_enabled=false时，在worker创建/发送前PERMISSION_DENIED(violations.field=network_enabled)，不计HTTPsequence、不fallback。pure正常计算。resource_group_handle只定位本Call已准入的微部署聚合资源组；监督器创建阻塞worker后调用HostService.AttachMicroWorker，核心实际入组Ack后才运行JS。每部署默认CPU250m/内存256MiB由cgroup聚合硬限额执行，单Call不复制额度；MPR不持有cgroup写权。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `call_id` | `string` | 按业务校验必填/默认值 |
| 3 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 4 | `micro_plugin_id` | `string` | 按业务校验必填/默认值 |
| 5 | `micro_version_id` | `string` | 按业务校验必填/默认值 |
| 6 | `micro_deployment_id` | `string` | 按业务校验必填/默认值 |
| 7 | `micro_generation` | `uint64` | 按业务校验必填/默认值 |
| 8 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 9 | `input_json` | `bytes` | 按业务校验必填/默认值 |
| 10 | `input_digest` | `string` | 按业务校验必填/默认值 |
| 11 | `http_bindings_json` | `bytes` | 按业务校验必填/默认值 |
| 12 | `resource_group_handle` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `result` | `InvokeResponse` | oneof outcome |
| 3 | `error` | `ErrorDetail` | oneof outcome |
| 2 | `diagnostics_json` | `bytes` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/micro.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
