# 扩展插件完整结构样例

[扩展插件](../README.md) · [接口目录](../../api/operations/extension-plugins/README.md)

> 规格版本：2.8.0；状态：可静态校验的设计样例，未安装、执行或发布为实际插件。

| 场景 | 固定宿主 | 模式及用途 |
|---|---|---|
| [普通宿主](ordinary/README.md) | 新版普通echo插件的text.rules点 | data：声明前缀规则，并允许下一层固定处理器扩展 |
| [微插件宿主](micro/README.md) | 显式声明text.rules的微版本 | data：ASCII大写规则，只读扩展数据，不取得网络能力 |
| [扩展链](nested/README.md) | 普通宿主上的前缀扩展 | skill：固定echo处理器，形成extension→extension→ordinary |

UUID、宿主镜像摘要及Blob ID是结构示例，必须替换为实际获准资源。贡献原始字节、扩展包、微宿主样例包、点契约及身份闭包的摘要均由本目录文件真实计算；这些摘要不代表对应镜像已构建或服务已部署。

[样例清单](index.json)用于逐包校验。[结构正反例](validation.json)检查三分宿主、四分Skill目标、不可变字段及有界Schema；[依赖图样例](graph-validation.json)检查跨类型DAG、handler环及16层宿主限制。

## 扩展点原始契约

- [text.rules输入](point-contracts/text.rules/input.schema.json)、[输出](point-contracts/text.rules/output.schema.json)、[流](point-contracts/text.rules/stream.schema.json)、[语义](point-contracts/text.rules/semantics.json)。
- [text.handler输入](point-contracts/text.handler/input.schema.json)、[输出](point-contracts/text.handler/output.schema.json)、[流](point-contracts/text.handler/stream.schema.json)、[语义](point-contracts/text.handler/semantics.json)。

先发布身份及不可变版本，再选择实际宿主创建disabled扩展部署；核对候选后按权限独立注册/启用Skill和扩展部署。宿主声明点、绑定成功和Prepare成功分别检查，不能直接拿样例UUID调用。

修改贡献字节或清单后须重算source/package摘要，再重算引用它的typed宿主与身份闭包/候选摘要。宿主重启或微资源设置改变只更新运行快照，不改身份闭包及公共Skill。静态校验不代替宿主适配器、排空、权限撤销或真实执行测试。
