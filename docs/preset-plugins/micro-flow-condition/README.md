# 流程条件微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `flow.condition`，根据结构化输入和声明条件返回判断结果，供上层流程选择分支。它不负责调度或实际执行分支；能力 ID 只是规划索引。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 和受审的有界声明条件解释器。
- 与[流程过滤微插件](../micro-flow-filter/README.md) 共用条件 AST；字段地址使用 RFC 6901 JSON Pointer。
- 不使用知识库 `match` / `topk` 查询语言，不接受 JavaScript、SQL、正则、用户函数或动态代码。
- 仅比较本次 JSON 数据；无文件、网络、密钥、当前时间或跨调用状态访问。

## 运行与启用门禁

复用 MPR，无独立 OCI。规划执行配置为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。条件 AST 另限深度 16、总节点 256。

MPR 现有 ECMAScript JSON 不自动包含该条件语言。解释器和路径解析须在不可变制品中受审打包，验证类型比较、缺失值和短路规则后才能发布；需要新的 Runtime 能力时必须显式扩展，不能使用 `eval` 补齐。

raw 契约通过[规划共享 Schema](../planned/shared.schema.json) 表达复用 AST，不是可直接安装的 MicroManifest；未来须先将有界 AST 展开为无 `$ref` 的受限业务 Schema，验证 Schema 预算及解释器门禁，无法通过时保持未部署。

未来创建微部署时核心才产生初始禁用的候选 Skill，固定微版本、微部署与摘要；无静态 `kind=capability` Skill 或裸服务。多次判断使用[异步 batch](../../skills/batches.md)。
