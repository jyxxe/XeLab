# HostService.CheckSkillAccess

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc CheckSkillAccess(CheckSkillAccessRequest) returns (CheckSkillAccessResponse);`

- 完整方法：`/xelab.v1.HostService/CheckSkillAccess`
- 允许对端：`plugin`；工作负载身份从通道验证，禁止正文自报。
- 授权：skill.invoke ∩ 目标原能力/HTTP动作 ∩ 插件代理权限 ∩ 当前主体/完整委托源链。
- 期限上限：2000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

skill_id和expected_definition_digest必填，验证实际mTLS实例、当前RpcDelegation与所有祖先/源凭证、固定Skill/安装审批及原目标调用权限；摘要失配CONTRACT_CONFLICT，缺权PERMISSION_DENIED。成功才返回当前ready/reason_codes，不包含地址/凭证。此检查不创建业务Call、额度reservation或执行模型，不代替实际Invoke时的输入/Blob/资源/当前授权检查；返回不是可复用授权票据。缓存每次命中及返回前为各依赖调用本方法；ready=false时不得以缓存绕过provider不可用。仅ResolveSkill的skill.read不能作为调用权限证明。 micro目标改为检查micro.invoke及父micro.runtime.use/固定微版本，不把运行时控制服务假作capability；ready依赖微部署及固定MPR。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `skill_id` | `string` | 按业务校验必填/默认值 |
| 3 | `expected_definition_digest` | `string` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `skill_id` | `string` | 按业务校验必填/默认值 |
| 2 | `definition_digest` | `string` | 按业务校验必填/默认值 |
| 3 | `ready` | `bool` | 按业务校验必填/默认值 |
| 4 | `reason_codes` | `string` | repeated |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/host.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
