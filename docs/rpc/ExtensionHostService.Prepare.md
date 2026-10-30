# ExtensionHostService.Prepare

[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)

> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。

`rpc Prepare(ExtensionPrepareRequest) returns (ExtensionPrepareResponse);`

- 完整方法：`/xelab.v1.ExtensionHostService/Prepare`
- 允许对端：`kernel`；工作负载身份从通道验证，禁止正文自报。
- 授权：extension.deploy/enable ∩ 每层真实宿主extension.host.use ∩ 固定物理root当前审批；仅核心mTLS控制。
- 期限上限：10000ms；仍取调用整体 deadline 的更小值。
- 幂等：方法可幂等处理，但仍须验证当前授权。

## 前置条件、执行及失败语义

仅向当前精确root OCI实例发送；logical_host必须与核心ExtensionDeployment.host一致，不为micro/extension虚构网络实例。核对版本/package/source摘要、<=256KiB严格UTF8 JSON contribution、ExtensionManifest、point契约/Schema、固定config及完整身份闭包。identity_chain_json符合ExtensionIdentityClosure，host_snapshot_json符合ExtensionHostSnapshot；二者及config各自限额、宿主链和联合强依赖图深度<=16并无环。host_chain_digest只覆盖不可变宿主/版本/包/point/handler定义身份，不含generation、实例句柄、健康或时间；动态snapshot另外绑定当前各层代次与root实例代次。核对宿主显式ExtensionHostDescriptor/profile与point允许模式；micro宿主须MPR1.3显式支持，并保留原MicroManifest输入/输出及副作用限制。祖先按拓扑顺序已准备，未准备/不兼容时拒绝，不静默绕过中间扩展。data/skill两模式均生成候选但不自动注册启用。只挂载/验证声明式数据，不执行业务/handler，不赋予Secret/网络/宿主代码加载能力。按extension_deployment/generation/root代次/身份闭包幂等返回有期限句柄；迟到响应由核心CAS丢弃。 逻辑host链仅从直接宿主到首个非extension宿主（ordinary或micro），每项是真实点绑定；micro→MPR仅由root部署/安装表示但仍参与联合依赖DAG。每边config_digest只取所属扩展部署不可变config，不含宿主可变resources/network/启停。host_chain_digest=SHA256(JCS(ExtensionIdentityClosure))，含leaf版本/包及固定root部署/安装。 prepared贡献/链元数据必须计入物理root已批准的有界缓存与父内存；micro根与MPR既有artifact_cache_max_bytes/entries共同计量，不为扩展另建无界缓存。不能容纳时明确RESOURCE_EXHAUSTED，不反复Prepare或扩容父预算。

## 请求字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `context` | `RpcContext` | 按业务校验必填/默认值 |
| 2 | `extension_plugin_id` | `string` | 按业务校验必填/默认值 |
| 3 | `extension_version_id` | `string` | 按业务校验必填/默认值 |
| 4 | `extension_deployment_id` | `string` | 按业务校验必填/默认值 |
| 5 | `extension_generation` | `uint64` | 按业务校验必填/默认值 |
| 6 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 7 | `source_digest` | `string` | 按业务校验必填/默认值 |
| 8 | `manifest_json` | `bytes` | 按业务校验必填/默认值 |
| 9 | `contribution_json` | `bytes` | 按业务校验必填/默认值 |
| 10 | `logical_host_json` | `bytes` | 按业务校验必填/默认值 |
| 11 | `identity_chain_json` | `bytes` | 按业务校验必填/默认值 |
| 12 | `host_chain_digest` | `string` | 按业务校验必填/默认值 |
| 13 | `config_json` | `bytes` | 按业务校验必填/默认值 |
| 14 | `root_instance_id` | `string` | 按业务校验必填/默认值 |
| 15 | `root_generation` | `uint64` | 按业务校验必填/默认值 |
| 16 | `host_snapshot_json` | `bytes` | 按业务校验必填/默认值 |

## 响应字段

| 字段号 | 字段 | Proto 类型 | 出现规则 |
|---|---|---|---|
| 1 | `prepared_handle` | `string` | 按业务校验必填/默认值 |
| 2 | `package_digest` | `string` | 按业务校验必填/默认值 |
| 3 | `host_chain_digest` | `string` | 按业务校验必填/默认值 |
| 4 | `observed_extension_generation` | `uint64` | 按业务校验必填/默认值 |
| 5 | `observed_root_generation` | `uint64` | 按业务校验必填/默认值 |
| 6 | `expires_unix_ms` | `int64` | 按业务校验必填/默认值 |

嵌套消息、enum、oneof 及字段注释见 [Proto](../contracts/proto/xelab/v1/extension.proto)。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。

## 错误与取消

校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。

任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。
