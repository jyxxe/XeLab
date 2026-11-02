# M2：Secret 托管与工作负载租约

[实施目录](README.md) · [密钥规范](../runtime/secrets.md) · [运行指南](../../crates/kernel/README.md)

状态：Secret 实现增量；不代表整个 M2 或生产恢复验收完成。卷、Blob、备份和
恢复由独立实现交付，不能从此页推断其可用。真实 OCI 新用例是否通过须检查
本次提交 CI 的专用步骤，本地 HTTP/数据库测试不能替代该证据。

## 管理链路

实现 createSecret、listSecrets、getSecretMetadata、rotateSecret、revokeSecret、
bindSecret、listSecretBindings、getSecretBinding、revokeSecretBinding。API 沿用
既有认证、实时授权、If-Match、幂等事务和审计；CLI 可直接调用这些 operationId。

- Secret 内容必须为合法 Base64，解码后 1～65536 字节。仅支持元数据查询；
  没有管理端原值读取接口。创建和轮换返回 Secret 元数据，可安全幂等重放。
- AES-256-GCM 使用随机 96 位 nonce；密文格式版本、Secret UUID、owner、
  Secret version 和主密钥标识共同认证。行交换、版本改写、密文篡改和错误
  主密钥均不能解密。主密钥标识为 SHA-256；不是主密钥明文。
- 主密钥从已有 master_key_file 读取，严格要求当前运行用户所有、0600、
  无符号链接、单硬链接及恰好 32 字节。Secret 明文不进入审计、outbox 或
  幂等缓存；写入指纹为带密钥的 HMAC，避免离线猜测短密钥。临时解码与
  解密缓冲在释放时清零。
- 轮换递增 version/revision；撤销后不能用 rotateSecret 恢复。绑定必须
  同时取得指定 Secret 的 secret.bind 和目标部署的 plugin.permission.grant。
  插件签名清单还须明确请求匹配的 secret.use；不接受其他宿主代理权限。
- 当前配置仅支持一个活动主密钥；Secret 轮换与主密钥轮换是两种操作。
  禁止直接替换主密钥文件，已有密文将不能解密。主密钥跨版本重加密尚未提供。

## HostService.LeaseSecret

生产 daemon 启动工作负载 mTLS 服务；服务证书在每次新握手时读取原子轮换
文件，避免长期监听持有过期证书。TLS 握手并发有界且限时，RPC 请求最多
64 KiB，每连接并发最多 16；租约处理限时 2 秒。

从真实客户端证书的唯一规范 SPIFFE URI 得到 instance UUID / generation；
拒绝 kernel 身份、正文身份冒用、旧代次、非活跃实例、停用/归档部署、失效
安装或已撤销发布者。RpcContext 的 subject_id 不赋予租约权。

租约还检查绑定和 Secret 当前状态、Secret 过期时间、签名清单请求范围、
原审批凭证的完整有效来源，以及审批者当前 secret.bind / 部署管理权。
授权共享锁将租约决定与撤销串行化。请求必须为 1～900 秒且不超绑定上限；
到期时间再受 Secret 到期约束。明文只返回给通过验证的工作负载。

当前采用保守的审批凭证生命周期：签发绑定的会话或 API Key 过期/撤销后，
该绑定不能再产生租约；需要撤销旧绑定并以有效凭证重新审批。绑定不是从
已失效凭证继续派生权限的通道，不能用新 Grant 自动替换失效的来源凭证。

租约表仅存 ID、绑定、实例代次、Secret version 及到期时间；审计记录实际
actor_instance_id。租约无明文重放缓存。到期或本地撤销不能强迫插件忘记
已经收到的第三方凭证，彻底收权仍需第三方轮换/撤销。

## 网络与升级

host_rpc_listen 必须使用专用 plugin CIDR 的首个主机地址；daemon 管理
带固定所有权标识的 xh-rpc dummy 接口，地址独立于任何插件实例。新实例
从第二个 /30 段分配。只允许实例源地址访问该 HostService 地址/端口，
其他宿主入口和转发继续拒绝。已有同名非受管接口或错误地址使预检失败。

升级前停机并备份 PostgreSQL 与独立保护的 master key，执行：

```sh
xelabctl migrate --config /etc/xelab/kernel.toml --target 3
```

v3 只增加内部 secret_binding_authority、secret_leases 和查询索引，不改
原始 metadata.sql 校验和或既有 API/Proto/配置 Schema。v1/v2 可显式升级，
重复迁移验证校验和；旧二进制拒绝 v3，禁止只切回旧镜像。历史上非此服务
创建的绑定没有审批来源，不能租约；先撤销再重新审批。恢复数据库时必须
配套正确主密钥；数据库备份本身不含主密钥。全平台防撤销回滚恢复尚需
备份恢复里程碑实现，不能用恢复旧数据库绕过最新撤销状态。

## 可重复验证

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 docs/tools/validate_docs.py --strict
```

设置 XELAB_TEST_DATABASE_URL 后 tests/secrets.rs 实际执行 PostgreSQL、HTTP、
mTLS，验证元数据输出、密文/主密钥错误、作用域拒绝、实例代次、轮换、
绑定及来源撤销；未设置 DSN 时的提前返回不能计作通过。

CI Python 镜像以 --request-secret 显式生成并签署 secret.use 请求，容器内
lease_probe.py 使用只读工作负载证书调用真实 HostService，验证初值、轮换
新版本及撤销后的拒绝。probe 只输出验证标记，不打印或保存 Secret 原值。
原有 AUTH-01～09、RUN-01/02、独立双插件与资源限额断言全部保留。

正式 RUN-03 仍待符合完整预算的授权独占宿主；此实现不覆盖 BACKUP-01、
主密钥轮换、灾难恢复或其他 HostService 方法，未支持的方法明确拒绝。
