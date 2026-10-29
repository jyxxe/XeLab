# JSON微脚本制品示例

[MPR目录](../README.md) · [动态Skill](../../../skills/micro-plugin-runtime/README.md)

> 规格版本：2.6.0；这些是设计样例，非已部署服务或已运行沙箱。

- [echo.js](echo.js)：通过虚拟stdin/stdout读取并回复JSON。
- [输入Schema](input.schema.json) / [输出Schema](output.schema.json)。
- [微清单](micro-manifest.json)、[真实源码/包摘要](artifact.json)。
- [发布版本](publish-version.json)、[创建微部署](create-micro-deployment.json)。
- [候选Skill](echo.skill.json)、[注册请求](register-skill.json)。
- [调用](invoke.json) / [业务输出](output.json)。

先创建微插件身份并上传源码Blob，再发布版本、创建固定微部署、注册候选Skill、启用微部署和Skill，最后调用；父MPR须已经批准并Ready。文件echo.js被打包为固定虚拟entrypoint main.js，不在宿主按调用者路径执行。

source_digest为原始echo.js字节SHA-256；package_digest为SHA-256(JCS({manifest:MicroManifest,source_digest}))。运行时OCI镜像签名和这份用户源码制品来源分开验证。输入输出仅JSON，source/manifest包含各自上限，stderr另行收集。
