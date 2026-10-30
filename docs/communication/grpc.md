# 插件 RPC 通用协议

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 传输与身份

所有 RPC 定义位于 [proto 目录](../contracts/README.md)。包名 xelab.v1。内核→插件与插件→HostService 双向验证当前代次工作负载身份，禁止普通插件直连其他插件内部端口。

RpcContext.request_id 必需 UUID；deadline_unix_ms 必须是未来时间，接收方取其、gRPC deadline 和方法最大期限最小值；delegation_token 在代理业务调用必需；生命周期与迁移只允许内核 peer。幂等键在有副作用方法必需。

delegation_token 为不透明句柄，目标插件信任经过认证的内核代理，而非离线解释句柄中的角色。subject_id/actor_instance_id 仅内核可信通道提供；HostService 仍从实际 peer 和句柄重新推导。详见 [委托上下文](../identity/delegation.md)。纯生命周期调用的主体字段允许为空。

## 字段校验

proto3 无法用 required 自动表达必填，接收方必须在业务入口校验：所有标识非空，除允许的 plugin_id/name 外为 UUID；版本 SemVer；digest 为 sha256；枚举 UNSPECIFIED 拒绝输入；计数上限 2^53-1。

input_json/output_json 必须为 UTF-8 且满足锁定契约，重复 key 拒绝；config_json 匹配清单 Schema；不接受任意未定义元数据。Proto 未知字段按向前兼容保留，但本版未知业务枚举拒绝执行。

## 期限和流控

Describe/GetHealth/Query/Resolve 默认 2s，上限 10s；Initialize 默认 30s，上限启动期限；Drain/Shutdown 受 grace_ms 限制，默认 30s、上限 300s；Invoke 默认 30s、上限 300s；Task.Execute 以步骤/任务期限为准，最长 24h 单步；迁移上限 1h。

JSON 输入最大 4 MiB、输出 16 MiB；流式业务块最大 1 MiB；Blob 块 256 KiB、总对象 64 MiB；流式待发送缓存最多 16 块且不超过 4 MiB，先触及者阻塞生产，持续 30s 无下游进展取消。

gRPC transport 的单消息收发上限配置为 20 MiB，以容纳 16 MiB 业务结果及信封；应用层仍执行各字段更小上限。方法细化期限以 rpc-semantics.json 为准；Drain/Shutdown 的传输上限为 grace_ms+2000ms，最长302000ms，不能因此延长业务排空时间。

取消须传播至生产者和其子调用。取消是协作通知，处理程序必须检查取消并停止可停止工作，不能视为外部操作回滚。[gRPC cancellation](https://grpc.io/docs/guides/cancellation/)

字段号删除后必须 reserved，不复用；不兼容变更新包 xelab.v2。完整方法语义和字段入口见 [逐 RPC 目录](../rpc/README.md)。
