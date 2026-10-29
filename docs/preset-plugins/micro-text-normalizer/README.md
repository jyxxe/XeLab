# 文本规范化微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `text.normalize`，用于在搜索、切块与 Embedding 前统一 Unicode、换行和指定空白规则。能力 ID 只用于规划分类，不代表可用服务或已注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 以及明确版本的 Unicode 规范化实现。
- 只执行声明的文本规则；不执行用户正则、代码、语言翻译或语义改写。
- 默认只归一换行；兼容性归一可能改变字符含义，只有显式选择 NFKC/NFKD 时执行。
- 不将规范化结果当作身份凭证比较、路径校验或 HTML 清理的替代方案。

## 运行与启用门禁

复用 MPR，无独立 OCI；规划为 `pure-json-v1`、`side_effect=none`。输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

MPR 目前保证 ECMAScript JSON，不自动承诺特定 Unicode 表和规范化 API。实现必须证明目标引擎提供固定语义，或在不可变制品内受审打包库；补齐前不得宣称所有规范化形式均可用。制品记录 Unicode 版本，升级随版本与摘要变化。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，并非可直接安装的 MicroManifest。未来发布前须展开为无 `$ref` 的受限业务 Schema，再通过运行库和预算门禁。

未来创建微部署后核心才产生初始禁用候选 Skill，绑定微版本、微部署和制品摘要；无静态 `kind=capability` Skill 或裸服务。多文本调用使用[异步 batch](../../skills/batches.md)。
