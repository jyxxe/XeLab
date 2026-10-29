# EmbeddingGemma业务Skill

[文档总目录](../../README.md)



- [org.xelab.embedding-gemma.multimodal-1.embed](embedding-gemma-multimodal-1.md)
- [org.xelab.embedding-gemma.text-1.embed](embedding-gemma-text-1.md)
- [org.xelab.embedding-gemma.text-8.embed](embedding-gemma-text-8.md)

## 批量执行

用户批量embedding默认走[核心异步batch](../batches.md)；每项仍遵守所选profile的items/token上限。text-8的8是实例活动并发上限，max_parallel受它和账户预算共同限制；同步兼容接口不自动改为202。
