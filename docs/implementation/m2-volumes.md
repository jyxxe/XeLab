# M2 单节点持久卷实现与验收边界

[实现目录](README.md) · [卷规范](../runtime/volumes.md)

当前实现接通 createVolume、getVolume、listVolumes、resizeVolume、deleteVolume，以及普通插件单副本的持久卷挂载。它依赖 Linux XFS project quota 的实际硬限制，不支持任意宿主路径；不支持的后端返回 503。完整 M2 仍需数据迁移、全平台备份恢复和正式 RUN-03，不能据此宣称生产就绪。

## 存储边界

`storage.root_path/volumes` 必须是操作员预先提供的独立 XFS 文件系统完整挂载根，启用 project quota accounting 与 enforcement，并设为 root:root 0700。内核以文件系统 UUID、数据库注册意图和私有持久身份标记绑定该命名空间；拒绝普通子目录、宿主同设备的其他挂载别名、符号链接、只读挂载和关闭强制执行的后端。其他管理程序不能修改此文件系统中的 project ID 或写入其他数据；主机 root 属于可信操作边界。

首次注册先持久化 preparing，再写原子标记并确认 ready；两处中断都可恢复。已有 ready 注册丢失或替换标记时拒绝重新认领。数据库及文件系统标记必须一起备份；挂载丢失后不能通过新建空目录或复制单个标记来恢复数据。每个卷的 UUID 容器目录保持 root 0700，只把继承相同 project 的 `data` 子目录交给插件 UID 65532。

创建、扩容在数据库中预留完整字节容量及 inode 元数据预算后才修改配额。容量必须按文件系统块对齐，只允许增长。每卷 inode 硬限制为 `clamp(capacity_bytes/4096, 256, 65536)`，inode 与字节配额均实际强制执行。预算另预留 inode 数量 × 实际 inode 大小 × 2 加 64 KiB，并保留文件系统全局 64 MiB 余量。容量池在首次绑定时确定；不支持在线缩小底层文件系统。删除后必须观察实际块与 inode 用量归零才释放预算，每个永久 project 身份保留 64 KiB 元数据预算。project ID 来自不循环的持久序列，删除后永不重用。

## 操作与恢复

创建接受后只暴露 Operation，完成实际配额、目录和身份检查后才发布 Volume。worker 每轮处理最多四个待恢复意图，并更新尝试顺序；单个卷清理不占用控制器协调循环。进程中断后重读同一 intent、UUID 和 project ID；匹配的已完成磁盘效果可以继续确认，未知项目或目录数据保留隔离。公开卷的失败不会触发自动删除。私有、尚未发布的失败创建可以清理自己的已确认数据。删除仅遍历已停止写入的受控 UUID 树，拒绝跨文件系统、挂载边界，且不跟随符号链接。

读取及后台观测使用实际 project quota 计数；字节或 inode 计数满额时状态为 full，并事务写入 `volume_full` outbox。配额与持久意图不符或后端不可验证时拒绝提供可用状态。XFS 的延迟分配预留可能在已用字节计数达到硬上限之前拒绝一次较大的写入；不能把此类分配失败直接等同于计数满额。写满错误由文件系统返回，内核不重试业务写入。后台每轮观测最多八个卷。

异步操作恢复会重新检查原始批准凭证及当前权限。凭证撤销或过期后，未发布创建隔离回收，已有公开卷保留。已经发生的扩容不逆向缩小或释放其预留容量；需要检查 Operation 和卷记录后处理。普通文件删除及 XFS inode 回收可以晚于 unlink，worker 在计数真正归零后完成 Operation。

## 挂载与写入权

DeploymentSpec.volume_ids 必须与签名清单的卷别名一一对应，scope、最低容量和读写能力必须匹配。当前实现要求部署操作者同时拥有卷并具备 volume.read；其他所有者卷的元数据读取权限不授予挂载写入权。启用部署同时记录凭证、来源地址及实例代次，控制器持续复核，且在发布 provider 前再次检查当前授权。批准凭证过期后需要使用有效凭证禁用、重新启用部署。

事务中锁定节点预算、命名空间、分配记录和卷；先提交 attachment、实例代次与单调 fence，再调用 OCI。一个 writer 排斥全部其他附件，多个只读附件最多 32 个。OCI spec 包含卷 fence 注解，已有实例必须与完整 spec 匹配。源目录需验证 project、UID、继承标记及准确的已批准配额；目标必须是镜像中已有的目录，每个路径组件不能含链接。禁止覆盖 /proc、/sys、/dev、/run、/tmp 和系统目录，禁止重复与嵌套挂载。

挂载使用 bind、nosuid、nodev、noexec 和明确的 ro/rw 标志；容器无 capabilities，seccomp 拒绝 quotactl、quotactl_fd 及清除 project 继承标记的 FSSETXATTR/SETFLAGS ioctl，包括 ioctl 高位被内核截断的变体。只暴露 data 子目录，不向插件暴露 namespace 标记或父目录。

禁用部署或全局启动恢复只有在 Runtime.stop 证实 task 退出、容器删除、cgroup 无进程后才释放附件。故障启动保留附件，下一次恢复仍使用原 fence；单凭进程观测或 SQL phase 不能把写入权交给另一个实例。禁用、程序卸载和容器重建均不删除卷。

显式删除另需 volume.delete、If-Match，并要求 `confirm_volume_id` 精确等于路径 UUID。确认值在幂等重放之前校验。仍有 attachment 或 backup/restore/migration hold 时返回 409。

## 迁移与测试

内部 schema 版本增加至 5，新增 0005_volume_service.sql，保留原始规格 SQL 及 v1～v4 校验和。升级须先停机并备份元数据及存储身份，再执行：

```sh
xelabctl migrate --config /etc/xelab/kernel.toml --target 6
```

不自动迁移、不降级。旧二进制拒绝新内部版本。M1 无卷部署不需要预先配置 XFS 挂载。

`tests/volumes.rs` 的普通 PostgreSQL/HTTP 回归验证不支持后端拒绝创建且无资源发布。隔离验收由 `tools/test-xfs.sh` 创建自己的全新镜像、文件系统和挂载，运行真实配额及卷 API/恢复/attachment/OCI 组件测试；绝不接收现有设备或生产目录。签名卷镜像由参考 Python 插件构建，容器探针验证实际写满、继承标记修改被拒、重建后的数据持久化和只读挂载。

本地当前环境无法直接运行宿主 XFS/OCI 验收。测试结果必须以对应提交的 CI 实际完成记录为准，忽略项不计作通过。共享只读的多实例应用行为、完整状态迁移、全平台异机恢复仍需进一步验收。
