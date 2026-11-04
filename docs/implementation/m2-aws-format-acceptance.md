# 真实 AWS 格式迁移组件演练入口

[实施目录](README.md) · [格式监督器](m2-format-transitions.md) · [备份任务](m2-backup-jobs.md)

本入口运行真实 AWS S3、签名 OCI、mTLS 与新建 XFS 项目的完整组件路径：
格式 1 初始化/业务写入 → 实际备份 → 格式 2 升级 → 保留新写入的不兼容备份回退。
当前未提供 AWS 凭证和授权测试宿主，**该演练尚未执行，编译和默认 CI 忽略不计为通过**。

## 必需环境

只在已授权、可销毁、无生产负载的独立 Linux 主机运行。需要 root、containerd/runc、
cgroups v2、XFS loop/prjquota、ctr、Docker、Rust 工具链与可创建一次性数据库的本机 PostgreSQL。
测试使用实际测得的宿主 CPU/内存进行准入，不降低既有内核/系统预算；建议独占 8 vCPU/32 GiB
或满足配置后仍有插件余量的更大宿主。此条件不等于已验证生产内核的 4C/16GiB 保留量。

| 配置 | 要求 |
|---|---|
| XELAB_TEST_DATABASE_URL | 独立测试 PostgreSQL，数字回环/本机 Unix socket，具有创建测试库权限 |
| AWS_REGION | 实际目标 bucket 的区域 |
| XELAB_BACKUP_CREDENTIALS_FILE | root 拥有的普通 0600 JSON；字段 access_key_id、secret_access_key、可选 session_token |
| XELAB_AWS_FORMAT_URI | 已存在测试 bucket 内明确授权的非空专用前缀，例如 s3://test-bucket/xelab-acceptance/ |
| XELAB_AWS_FORMAT_NAMESPACE | 新建的 xelab-aws-format- 开头的专用 namespace；不得有既存工作负载 |
| 两个 IMAGE 环境变量 | 下述实际导入 containerd 的固定平台 manifest digest 引用 |

凭证须允许受管前缀中的读取、条件写入和分片上传/完成/中止、必要的分片列举；不创建或删除
bucket。不要在仓库、命令行或日志中放明文凭证。保持已配置的代理和 CA 信任；这个演练不接受
XELAB_TEST_S3_ENDPOINT（包括空值），不能通过兼容后端或改写 off_host 来替代 AWS。

## 制备专用制品

以下示例假设 x86_64 主机。先设置专用 namespace，确认目录不存在后生成测试制品。
格式 1 制品观察任何 Apply 调用；格式 2 制品在实际转换落盘后故意返回未知 RPC 结果。
这些参数包含在实际签名清单中，不是内核权限、备份或格式校验的绕过开关。

```bash
export XELAB_AWS_FORMAT_NAMESPACE="xelab-aws-format-$(python3 -c 'import uuid; print(uuid.uuid4())')"
sudo ctr namespaces create "$XELAB_AWS_FORMAT_NAMESPACE"
test ! -e examples/generated-aws-source
test ! -e examples/generated-aws-target
python3 examples/echo/build_artifact.py \
  --publisher-id 00000000-0000-4000-8000-000000000001 \
  --output examples/generated-aws-source --implementation python \
  --volume-access rw --data-schema 1 --migration-observe-apply
python3 examples/echo/build_artifact.py \
  --publisher-id 00000000-0000-4000-8000-000000000001 \
  --output examples/generated-aws-target --implementation python \
  --volume-access rw --data-schema 2 --plugin-version 3.0.0 \
  --migration-fail-after-write
docker build -t docker.io/xelab-aws/source:test \
  --build-arg ARTIFACT_DIR=examples/generated-aws-source \
  -f examples/echo-python/Dockerfile .
docker build -t docker.io/xelab-aws/target:test \
  --build-arg ARTIFACT_DIR=examples/generated-aws-target \
  -f examples/echo-python/Dockerfile .
docker save docker.io/xelab-aws/source:test -o examples/generated-aws-source/image.tar
docker save docker.io/xelab-aws/target:test -o examples/generated-aws-target/image.tar
sudo ctr -n "$XELAB_AWS_FORMAT_NAMESPACE" images import examples/generated-aws-source/image.tar
sudo ctr -n "$XELAB_AWS_FORMAT_NAMESPACE" images import examples/generated-aws-target/image.tar
sudo ctr -n "$XELAB_AWS_FORMAT_NAMESPACE" images list
```

核对输出中的平台 manifest digest，将两个镜像分别添加对应的不可变名称，再设置
XELAB_TEST_AWS_FORMAT_SOURCE_IMAGE、XELAB_TEST_AWS_FORMAT_TARGET_IMAGE 为
docker.io/xelab-aws/source@sha256:实际摘要 和 docker.io/xelab-aws/target@sha256:实际摘要。
不使用 image index 摘要或可变 tag 作为实际测试输入。

## 执行与检查

```bash
cargo test -p xelab-kernel --test volumes --no-run --locked
sudo -E bash tools/test-aws-format.sh --isolated-test-host \
  /absolute/path/to/target/debug/deps/volumes-<actual-hash>
```

脚本确认测试二进制确实包含目标用例，只创建新的 512 MiB loopback XFS 镜像，不接受
已有设备或 mount。Rust 用例取得真实宿主/数据库租约，在专用 namespace 内通过实际签名
及镜像字节检查登记内部组件制品；发布者须由当前 Owner 经实际 API 明确重新信任。
公开版本化制品入口仍关闭，这不是公开安装验收。

仓库经真实 AWS 条件标记发布/读回后就绪。备份必须由 createBackup 和真实后台传输生成
ready/off_host=true；没有直接 SQL 提升备份或格式状态。升级必须只派发一次 Apply RPC，
实际提交业务写入但丢失 RPC 成功结果后停止并 Verify。回退必须恢复老备份到新的卷 UUID，
目标原程序不得收到逆向 Apply；原格式 1 数据及升级后格式 2 的新写入均须保持。
两次切换后执行实际 Broker 写入，最终停止所有测试工作负载并核对 CPU/内存预留归零。

仅所有断言成功才保存 completed.json；脚本要求该文件非空，不能把零测试执行当作成功。
新随机主密钥、started.json 和成功收据保存在 root 0700 的独立证据目录中，文件为 0600。
脚本清理本次新建的本地镜像/挂载；远端测试前缀、加密对象、证据/主密钥及一次性元库保留供复核，
由已授权运维者另行清理。不会删除整个 bucket 或既有前缀；失败挂载不能卸载时明确保留。

本用例是实际 AWS 支持的内核组件演练入口，仍不替代独立宿主整体恢复、生产 xelabd 的
资源预留压力验证、RUN-03 或完整 BACKUP-01。当前缺少资源，任何上述成功结论均待实际执行记录。
