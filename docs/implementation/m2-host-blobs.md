# M2 工作负载 Blob RPC

[实现目录](README.md) · [Blob 存储](m2-blobs.md) · [RPC 委托](../identity/delegation.md)

本增量接通 HostService.ReadBlob、WriteBlob，复用 HTTP Blob 的私有目录、持久根身份、实际容量预留、摘要校验、发布及回收机制。RPC 工作负载来自 CA 验证的 mTLS 客户端证书；请求正文不能替换实例或代次。完整 M2 的数据迁移、平台备份恢复和正式 RUN-03 仍待完成。

## 授权范围

插件签名清单可以申请可选、无条件的 blob.read/blob.create，资源必须为 blob/all；required=true 仍不支持。默认普通插件不获得 Blob 代理权限。管理员需要在停用部署时通过 approvePlugin 显式批准 Blob 规则；当前子集只支持无条件 Blob 规则和空 egress。批准必须处于管理员当前权限范围内，并保存该批准的原始 Grant 来源、凭证及来源地址。

调用开始时，Broker 对输入 Schema 中明确标有 `x-xelab-resource-kind: blob` 的 UUID 逐个检查当前 blob.read 与插件批准范围，形成最多 256 个源 Blob 的精确白名单。支持 properties、items 和对象 additionalProperties 的结构遍历；不在任意字符串中猜测 UUID，也不把未标记字段作为读取权限。含资源标记的联合、条件或引用结构不支持，拒绝使用含糊的数据范围。

Call 的 ExecutionDelegation 固定原始 capability.invoke 和获准 Blob 范围，RpcDelegation 另绑定实际实例、代次及 Call。Host 每次检查句柄 HMAC 摘要、活动 Call/执行委托/工作负载、启用且未归档部署、已安装制品及可信发布者，并核对正文中非空 subject/actor。终止、过期、撤销或失去实际实例身份的句柄不能继续使用。

每次 Blob 访问既检查当前凭证权利，也检查调用者与审批者的原始 Grant 替代来源、当前选择器和条件。撤回原来源后，即使后来获得相同动作的新 Grant，也不能恢复旧委托。写入只创建执行主体所有的 Blob；读取还需位于本次输入白名单。刚创建且属于同一主体的输出 Blob，不自动进入同一调用的读取白名单。

## 传输与持久化

ReadBlob 只读完整对象，先验证登记长度及 SHA-256，再以连续 offset 和最多 64 KiB 的帧发送。每块发送前重新检查当前源链、批准、Call 状态、白名单及 Blob 生存状态；失效中断流。无 range 接口。空 Blob 返回空流。

WriteBlob 首帧必须为唯一 header，之后只能是非空 data；校验连续 offset、每帧最多 256 KiB、最多 16384 个 data 帧，以及完整长度和摘要。单对象 0～64 MiB。content_type、request_id、幂等键及上下文都有边界检查；零字节对象允许仅 header 后结束流。首帧等待最多 2 秒，传输策略最多 10 秒，始终取 gRPC、RpcContext 和 Call 剩余期限的更小值。

上传开始和发布前分别复核授权，不在慢速传输期间持有授权事务锁。文件 fsync、非覆盖发布、Blob 元数据、Call 输出账本和成功审计在受保护的提交流程中完成。发布后遇到不确定 COMMIT 不主动删除文件；GC 通过与上传事务相同的数据库对象锁核实元数据，避免删除已提交对象。未发布临时文件由 Drop 清理。

幂等键按 Call 固定，实际字节每次重传都重新计算摘要；相同键不同指纹拒绝。每 Call 最多 16 个输出对象，重复请求不新增记录。Call 仍有效时返回原 BlobReference，原对象已删除或过期则明确报错。Call 结束后旧句柄不提供重放授权。内部 host_blob_outputs 保存元数据结果账本；它不构成可公开检索任意输出的接口，未知结果不能盲目再次创建。普通下载仍需当前 HTTP blob.read。

全局最多 16 个 RPC 读取、4 个 RPC 预留 worker；RPC 与 HTTP 共享每 App 的 4 个活动上传额度。取消后的阻塞文件验证/预留仍持有对应 worker 额度，直到实际磁盘操作结束。期限覆盖初始化、数据库检查及已返回流的后续读取。错误附带不含令牌/正文/路径的 xelab-error-bin。

## 内部迁移与验证

内部版本增加至 6，新增 0006_host_blob.sql 的批准来源、Call 数据范围及输出账本；原规格 SQL/OpenAPI/Proto 及先前迁移校验和保留。升级需先停机备份，再显式执行：

```sh
xelabctl migrate --config /etc/xelab/kernel.toml --target 6
```

本地真实 PostgreSQL、HTTP 和 mTLS 测试覆盖 0/64 MiB、错误帧/offset/长度/摘要、重放冲突、输入范围、正文冒充、旧代次、Call 终止、流中撤权、上传中撤权且不阻塞授权事务，以及调用者/审批者来源不可由新 Grant 代替。此类测试的工作负载元数据为隔离 fixture。

OCI 验收另构建独立签名 Blob 插件契约，经实际安装、批准、启用、Broker Call 委托、容器到 HostService 的 mTLS 读写，以及 HTTP 下载/Call 重放验证完整链路。该宿主验收须以对应提交实际 CI 结果为准；不能用本地忽略项代替通过。
