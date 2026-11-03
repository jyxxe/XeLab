# M2：不可变 Blob 与可恢复清理

[实施目录](README.md) · [存储规范](../runtime/volumes.md) · [运行指南](../../crates/kernel/README.md)

状态：HTTP/CLI Blob 存储实现增量。后续 [HostService Blob RPC](m2-host-blobs.md)
和 [持久卷](m2-volumes.md) 独立交付；
一致性备份及恢复仍需独立交付，不据此标记整个 M2 完成。

## 已接通的链路

实现 uploadBlob、getBlob、downloadBlob、deleteBlob，沿用现有 OpenAPI。
内容最多 64 MiB，允许空对象，默认保留 24 小时。平台生成对象路径，公开
接口不能指定宿主路径或改变已有内容。元数据与文件摘要不包含内部路径。

上传必须提供 Content-Length、X-Content-SHA256、Idempotency-Key，以及
content_type 查询参数。正文为 application/octet-stream；拒绝编码转换、
混用 Transfer-Encoding 和额外参数。服务核对实际接收字节数及 SHA-256，
不以客户端声明的摘要替代内容检查。相同幂等键重放也重新校验实际正文。

- 收取正文前鉴权，传输结束后再次读取当前凭证/完整来源/授权。上传过程
  不占住授权共享锁，因此撤权无需等待慢上传结束；撤权后不能发布元数据。
- 服务同时最多 4 个上传，正文读取限时 10 秒。底层先预分配实际磁盘空间，
  以目录锁串行化容量检查，保留至少 128 MiB 空闲余量；该保护不等于持久卷
  的 XFS project 硬配额，也不能替代宿主其他服务的容量管理。
- 临时文件从创建开始为运行用户所有、0600、单链接。逐层 openat/O_NOFOLLOW
  检查目录；最终数据根必须私有。只接受内部 UUID 名称，拒绝链接、错误
  所有者/权限、跨设备文件。生产 daemon 使用 root；本地测试使用独立私有目录。
- 数据 fsync 后原子且不覆盖地发布文件，数据库再原子提交元数据、幂等
  响应与审计。在元数据提交前，该文件不能被用户列表/下载接口访问。
- 元数据 COMMIT 的连接失败可能表示已提交。已发布文件不会由 Drop 删除；
  文件锁与数据库对象锁共同隔离在途发布和清理，避免断链后 GC 误判未提交。
  未发布临时文件在失败时删除，进程崩溃残留由后续扫描处理。

getBlob 仅返回元数据。downloadBlob 先在受控线程验证文件长度/完整摘要，
再以有界块传输，每块重新检查当前授权与对象有效期。正文固定为二进制附件，
带 no-store / nosniff / SHA-256，不能用上传的 MIME 在管理源内执行 HTML。
源链失效、删除或到期会终止尚未完成的流；已接收的字节无法撤回。

首次使用先在数据库提交存储根注册意图，再原子写入私有身份标记并确认 ready。
重启可恢复“意图已提交但标记未写”和“标记已写但确认未提交”两种中断。
已注册根缺少标记或身份不匹配时拒绝上传、读取和清理，不把丢失挂载后的
空目录重新当作数据根。备份必须同时保留身份标记与注册元数据。

## pin、到期、删除与崩溃恢复

pin_count>0 或仍被 attempts.checkpoint_blob_id 引用的对象，即使过期也保留。
删除需要独立 blob.delete、If-Match 和幂等键，处于 pin/检查点引用时返回 409。
任务引擎的 pin 生命周期仍由任务里程碑接入；本实现不伪造任务运行能力。

删除先在同一数据库事务移除公开元数据并写入 blob_deletions 意图。后台只对
已提交意图删除文件，文件已不存在时仍可完成。不存在“先 unlink、事务失败后
元数据却复活”的路径。过期 GC 使用相同流程，并再次通过行锁/FK 检查保护引用。

后台每轮最多处理 16 个过期对象、16 个删除意图和 16 个目录项。目录扫描
保留迭代位置，过旧的私有临时文件/无元数据文件才进入候选；未知名称或
异常对象保留隔离。清理持有候选文件描述符并核实 inode，不能跟随链接。
发布者的数据库对象锁在 PostgreSQL 决定 COMMIT/ROLLBACK 前保持，GC 不能
只凭客户端已断开或文件句柄已关闭就删除。后台不在 Controller 健康循环内做
同步磁盘工作，错误记录为待清理，后续重试；不是所有磁盘错误都能自动修复。

## CLI

```sh
xelabctl api uploadBlob --endpoint https://node.example.org \
  --ca-file ca.crt --token-file token \
  --body-file payload.bin --query content_type=application/octet-stream \
  --idempotency-key stable-upload-key
xelabctl api downloadBlob --endpoint https://node.example.org \
  --ca-file ca.crt --token-file token --path blob_id=<UUID> \
  --output-file received.bin
```

CLI 流式读取普通文件并生成长度/摘要；二进制上传不接受未知长度 stdin。
下载要求 output-file，先在同目录的 0600 临时文件验证长度/摘要，再原子发布；
目标已存在（包括符号链接）即失败，不覆盖。错误/部分响应不发布为完整文件。
目录持久化失败可能留下已验证的最终文件，需要检查；不静默覆盖后重试。

## 升级和验证

升级前停机备份 PostgreSQL、独立保护主密钥及 Blob 内容，执行：

```sh
xelabctl migrate --config /etc/xelab/kernel.toml --target 6
```

v4 添加内部删除意图/存储根注册表、过期/检查点索引和非负 pin 约束，原始 metadata.sql
及 v1～v3 校验和保持不变。升级幂等；发现未知版本、旧校验和差异或负 pin
数据时拒绝。旧二进制不能读取 v4，不以只切镜像方式回退。此迁移不提供全平台
恢复或防撤销回滚；不能把单独复制数据库当作完整备份。

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 docs/tools/validate_docs.py --strict
```

设置 XELAB_TEST_DATABASE_URL 后，tests/blobs.rs 实际运行 PostgreSQL、私有文件、
HTTP 与 HTTPS CLI：0/64 MiB 边界、短/长/断开上传、错误摘要与重放、上传中
撤权、作用域隐藏、篡改文件拒绝、pin/过期、延迟删除、文件已删但意图未完成、
已发布但事务未确定的清理围栏、根标记丢失/替换拒绝、注册中断恢复、
孤儿回收及 CLI 不覆盖。底层测试另检查链接、
权限和临时文件释放。未设 DSN 的提前返回不能计作集成通过。

所有故障注入只触碰用例新建的数据库与临时目录。正式 RUN-03、真实 XFS
配额、OCI 及备份恢复仍分别报告，不能用本页用例代替。
