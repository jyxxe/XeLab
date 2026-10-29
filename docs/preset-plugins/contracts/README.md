# 预设共享业务契约

[预设目录](../README.md) · [机器注册表](registry.json)

- [文本embedding输入](embedding-text/input.schema.json) / [输出](embedding-text/output.schema.json)
- [多模态embedding输入](embedding-multimodal/input.schema.json) / [输出](embedding-multimodal/output.schema.json)
- [知识库逐接口](../knowledge-base/interfaces.md)，各自contracts子目录保存input/output/stream/semantics。

契约摘要使用平台固定原始文件算法，注册表和所有清单/Skill副本必须一致。

- [对象存储逐接口](../object-storage/interfaces.md)，6个固定能力。
- [网页搜索微业务契约](../web-search/contracts/search/input.schema.json) / [输出](../web-search/contracts/search/output.schema.json)，只由核心生成micro Skill，不声明裸capability提供者。
