# ExtensionHostService.Invoke

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Invoke(ExtensionInvokeRequest) returns (ExtensionInvokeResponse);`

- 完整方法：`/xelab.v1.ExtensionHostService/Invoke`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：skill.invoke ∩ leaf extension.invoke ∩ 每层真实宿主extension.host.use ∩ 当前root审批；micro root另micro.invoke/runtime.use；实际handler Skill/目标权限及原来源链。
- 期限上限：30000ms；仍取调用整体 deadline 的更小值。
- 幂等：不保证方法无副作用，按业务契约和稳定幂等键判断，禁止 SDK 默认自动重放。

## 前置条件、执行及失败语义

仅当前核心Call能调用固定prepared_handle。核对extension generation/package/host_chain_digest、input_digest和point输入Schema，execution_chain_json按ExtensionHostSnapshot冻结每层UUID/版本/包/generation、root实例/代次和handler定义摘要；每次检查当前启用、审批/撤权与完整祖先链。data模式调用宿主明确声明的数据适配器，以已核验contribution扩展处理并返回point.output；不得eval/import原数据。skill模式只由监督器经已有HostService.InvokeSkill调用manifest固定handler_skill_id/definition_digest，核心再次检查handler当前权限、精确目标、同一根deadline与调用祖先无重入，不得以扩展owner/宿主身份代替原调用者。VM没有HostService/RPC入口。副作用按point、handler及原root profile保守合成，不可通过扩展将non_repeatable降为none或扩大pure网络/文件/Secret能力。微root沿原MicroExecutor资源组/Attach和JS子限额执行，默认250m/256MiB共享，不复制父4C4GiB预算；network_enabled=false对HTTP执行仍拒绝且无fallback。所有业务经核心Skill，不支持扩展业务流；输入/输出受point、祖先和物理root最小上限，micro各<=1MiB。结果/error恰一，gRPC OK不是业务成功。Cancel/QueryResult复用CapabilityService；停止/撤权传播到固定handler Call，已发生外部副作用不撤回，unknown优先于正常输出且禁止盲重放。 ordinary root的扩展点准入使用专用extension.host.use及当前审批，不强求无关capability.invoke；只有实际另调已注册能力才按原动作检查。宿主仅提供点时不隐式执行全部祖先Skill，也不因此要求所有祖先extension.invoke；某祖先handler实际被执行时才检查它的Skill/invoke授权。 扩展点输入/输出JSON各最多1MiB、总期限最多30000ms，再与各点/handler/root更低预算求交；contribution/config/业务input同名控制字段不能覆盖固定handler、主体、宿主、代码或委托。 micro物理执行根必须携本ExtensionCall的resource_group_handle；核心先将可信root MicroPlugin/Version/Deployment/package/generation/resources/network写入Call.micro_*执行快照，再经Runtime.configure_micro_group按该微部署聚合准入，监督器使用此handle经HostService.AttachMicroWorker实际入组，只有Ack后运行。ordinary根要求handle为空，不得把任意MPR租约用于普通root。MPR从已Ready微prepared registry按identity闭包中的真实micro版本/部署/package及当前generation读取原代码/limits/resources/point，不从扩展contribution/input取得源码或替换root版本；一个物理执行只建立一个worker/准入，不额外调用MicroRuntimeService.Invoke复制执行或额度。 skill模式须宿主当前已批准的skill.invoke代理规则，具体资源限定到闭包handler；不批准不启用该模式，不把mode声明当授权。micro VM仅能按新版本声明读取固定data投影，不取得代理客户端。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `call_id` | `string` | 按业务校验必填/默认值 |
| 3 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 4 | `extension_deployment_id` | `string` | 按业务校验必填/默认值 |
| 5 | `extension_generation` | `uint64` | 按业务校验必填/默认值 |
| 6 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 7 | `host_chain_digest` | `string` | 按业务校验必填/默认值 |
| 8 | `input_json` | `bytes` | 按业务校验必填/默认值 |
| 9 | `input_digest` | `string` | 按业务校验必填/默认值 |
| 10 | `execution_chain_json` | `bytes` | 按业务校验必填/默认值 |
| 11 | `resource_group_handle` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `result` | `InvokeResponse` | oneof outcome |
| 2 | `error` | `ErrorDetail` | oneof outcome |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/extension.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
