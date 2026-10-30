# JSON微脚本制品示例

[MPR目录](../README.md) · [动态Skill](../../../skills/micro-plugin-runtime/README.md)

> 规格版本：2.8.0；这些是设计样例，非已部署服务或已运行沙箱。

- [echo.js](echo.js)：通过虚拟stdin/stdout读取并回复JSON。
- [输入Schema](input.schema.json) / [输出Schema](output.schema.json)。
- [微清单](micro-manifest.json)、[真实源码/包摘要](artifact.json)。
- [发布版本](publish-version.json)、[创建微部署](create-micro-deployment.json)。
- [手动配置CPU/总内存/联网](settings.json)：停用并排空后提交settings；创建缺省250mCPU/256MiB，联网关闭。
- [候选Skill](echo.skill.json)、[注册请求](register-skill.json)。
- [调用](invoke.json) / [业务输出](output.json)。

先创建微插件身份并上传源码Blob，再发布版本、创建固定微部署、注册候选Skill、启用微部署和Skill，最后调用；父MPR须已经批准并Ready。文件echo.js被打包为固定虚拟entrypoint main.js，不在宿主按调用者路径执行。

source_digest为原始echo.js字节SHA-256；package_digest为SHA-256(JCS({manifest:MicroManifest,source_digest}))。运行时OCI镜像签名和这份用户源码制品来源分开验证。输入输出仅JSON，source/manifest包含各自上限，stderr另行收集。

新微部署默认resources为250mCPU/256MiB、network_enabled=false，资源在同部署所有活动worker之间共享；纯echo不需要联网。手动调整使用核心settings接口，先停用排空、携If-Match提交，再重新启用；不会为补默认设置重算上述源码/包摘要，也不改变候选Skill。

资源与联网属于微部署设置，不写入不可变微清单；手动修改不会更改这些样例的source_digest/package_digest或候选Skill。Prepare/Invoke绑定当前deployment generation。旧部署记录迁移时显式补充默认资源及network_enabled=false，已有HTTP部署须重新确认开启，不能由历史profile默认为true。
