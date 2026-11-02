# M2-A：XFS 硬配额后端

[文档总目录](../README.md) · [卷规范](../runtime/volumes.md)

状态：首个后端实现，待 PR 审查；不是完整 M2 持久卷服务。管理 API 的
createVolume、挂载授权、volume_fence、删除及恢复仍未接入，普通插件继续
拒绝持久卷。此提交独立基于 M1 main，不包含未合并的 Controller 修复。

## 已实现边界

- `xelabctl storage-check --config /etc/xelab/kernel.toml`：只读检查配置中
  `storage.root_path/volumes`，不创建目录、不格式化、不挂载、不启用配额。
- `storage::xfs::ProjectStore`：创建新的 UUID 目录、设置及回读字节/inode
  硬配额、读取真实使用量、扩容、关闭后重新打开并校验原 project identity。
- 使用 `quotactl_fd`、目录文件描述符及 XFS ioctl，不调用 shell 或解析设备路径。
  逐层 `openat(O_NOFOLLOW)`，祖先必须 root 所有且不可由组/其他用户写入，
  后端根和卷根仅 root 可访问；符号链接和跨文件系统卷目录被拒绝。
- XFS 必须同时启用 project accounting 和 enforcement。每次操作重新检查，
  不能将 `pqnoenforce` 当成可用后端。非 XFS、缺失内核支持、realtime
  数据池和带 project/realtime 继承的后端根均拒绝。
- 容量以文件系统块对齐，内部转换使用 512 字节 quota basic block；不向上
  舍入容量。硬/软限制相同，不依赖 grace period；调用者必须同时给定 inode
  预算。该内部约束不修改 2.8 VolumeSpec，API 层容量及 inode 策略仍待评审。
- project ID 必须非零且不超过 `i32::MAX`。已有使用量或任何非零限额的 ID
  不能重用；删除文件释放使用量，不清除 ID 预留。扩容不能缩小 inode 限额。
- 后端根的独占 `flock` 防止多个适配器并发分配；一个 handle 的变更要求
  Rust 可变借用。操作完成后 `syncfs` 并回读校验；错误不泄露宿主路径。

## 操作员预检

先由宿主管理员在专用数据文件系统准备 XFS project quota，并挂载于
`storage.root_path/volumes`。不使用现有生产设备练习格式化；此命令没有
创建或修复文件系统的权限代理功能。所有祖先目录不可由普通用户写入，
卷根为 root:root 0700，根目录 project ID 为 0 且不继承 project。

```sh
cargo build --workspace --bins --locked
sudo target/debug/xelabctl storage-check --config /etc/xelab/kernel.toml
```

成功输出包括 `backend=xfs_project`、真实块大小、记账和强制执行状态。
失败返回非零退出码；不会将节点或任何 Volume 标记为 available。
此预检不证明磁盘预算、完整挂载隔离、备份或 M2 验收已经满足。

## 可重复的真实文件系统测试

仅在授权的隔离 Linux VM 或 GitHub Actions 临时 runner 运行。需要 root、
Linux 5.14+ 的 `quotactl_fd`、XFS/quota 内核支持、loop mount、xfsprogs、
至少 1 GiB 临时磁盘余量。脚本只新建两个 512 MiB 镜像，不接收已有设备
或挂载路径。结束时先卸载再删除镜像；卸载失败则保留现场并返回失败。

```sh
cargo test -p xelab-kernel --test xfs --no-run --locked
# 从上一命令的 Executable 输出复制测试二进制的完整路径：
sudo env XELAB_XFS_TEST=1 bash tools/test-xfs.sh \
  --isolated-test-host target/debug/deps/xfs-<hash>
```

脚本分别挂载 `prjquota` 与 `pqnoenforce`，显式运行 ignored 特权用例；
后者不能通过预检。测试实际写入及 fsync 到 EDQUOT/ENOSPC，验证嵌套目录
继承、第二卷仍可写、跨 project 硬链接失败、扩容后可继续写、拒绝缩容、
重开后限额保留、删除内容释放用量、inode 耗尽、ID 不复用、路径和权限
检查，以及运行期间关闭 enforcement 后已有 handle 立即拒绝后续操作。

Kernel CI 保留原有 PostgreSQL 和 OCI 用例，并独立执行此真实 XFS 步骤。
默认 `cargo test` 的 ignored 输出不能算该项通过；必须核实 Actions 中
`Real XFS project quota enforcement` 步骤成功。本地受限容器没有 mount
能力时只运行单元测试，不伪造文件系统验收结果。

## 恢复、迁移和剩余工作

本 PR 不改 SQL、Proto、JSON Schema 或公开 API，无数据库迁移。
后续服务必须在外部副作用前持久化创建意图、永久分配唯一 project ID、
锁定所有者/容量预算并建立数据库与后端映射。创建中断可能留下私有空目录
或已预留 quota；适配器故意保留这些对象，不自动清零限额或递归删除。
需要显式恢复/隔离流程确认身份后才可发布，不可将重试成功等同事务完成。

仍需独立 PR：授权和资源账本事务、迁移与兼容测试、挂载路径策略、实例
代次和单写 fencing、容器权限与 ioctl 防绕过验证、删除确认与安全清理、
卷满事件、故障注入和端到端恢复。`syncfs` 可能等待整个文件系统，不能
直接放入要求低延迟的 Controller tick；未来服务需有界阻塞工作队列。

当前文件系统用例仅覆盖 RUN-04 的硬配额基础，不能宣称 RUN-04、RUN-05、
RUN-06、BACKUP-01 或 M2-A 全部通过。Blob、Secret、备份恢复尚未实现。

## 实现依据

- [quotactl / quotactl_fd](https://man7.org/linux/man-pages/man2/quotactl.2.html)
- [XFS inode 属性](https://man7.org/linux/man-pages/man2/ioctl_xfs_fsgetxattr.2.html)
- [XFS geometry](https://man7.org/linux/man-pages/man2/ioctl_xfs_fsgeometry.2.html)

Rust UAPI 布局有大小/偏移检查，真实 ioctl 测试验证宿主 ABI。CI 当前验证
Linux x86_64；其他体系结构未经本 PR 的真实文件系统验收。
