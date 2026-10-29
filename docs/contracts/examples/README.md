# 可校验样例

[文档总目录](../../README.md) · [机器契约](../README.md)

| 文件 | 模型与用途 |
|---|---|
| [plugin-manifest.json](plugin-manifest.json) | Manifest；无状态 tool.echo 插件 |
| [echo/](echo/) | tool.echo 的 input/output/stream/semantics，摘要已按固定算法计算 |
| [deployment.json](deployment.json) | DeploymentSpec；500m CPU / 256MiB 示例，尚未启用 |
| [permission-group.json](permission-group.json) | CreateGroup；仅指定 document.parse 能力 |
| [child-account.json](child-account.json) | CreateChild；selected 部分继承 |
| [api-key.json](api-key.json) | CreateApiKey；全部快照及固定有效期/调用上限 |
| [workflow.json](workflow.json) | WorkflowSpec；echo 单步骤，引用样例契约摘要 |
| [kernel.toml](kernel.toml) | 内核及系统分别预留，需真实宿主容量/证书/存储支持 |
| [http-requests.json](http-requests.json) | 每个带结构请求体接口的最小结构样例 |
| [index.json](index.json) | 上述手工 JSON 样例到 Schema 的校验映射 |

示例 UUID、域名和时间需要替换成实际部署对象；结构校验成功不等于获得授权或具备真实 OCI 镜像。不存在“已发布 echo 插件”或可直接安装的假镜像地址。

plugin-manifest 的 schema_digest 对应本目录 echo 文件的实际摘要；构建真实制品后，镜像摘要、原始清单字节及签名还需重新计算。

UI与批处理样例：[batch提交](skill-batch.json)、[用户主题偏好](ui-preferences.json)、[插件窗口/组件声明](plugin-ui.json)。UI贡献为待开发示例，未将其加入实际预设清单。
