# org.xelab.embedding-gemma.multimodal-1.embed

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

Skill定义：[原始JSON](embedding-gemma-multimodal-1.skill.json)。目标 `org.xelab.embedding-gemma.multimodal-1` / `ai.embedding.multimodal@1.0.0`。输入输出identity映射，无流式；响应业务对象位于SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `items` | 是 | 单项 |
| `purpose` | 否 | 任务用途，由插件固定映射前缀；default="semantic_similarity" |
| `dimensions` | 否 | 先保留前 d 维再 L2 归一化；default=768 |

每个嵌套字段的类型和限制见 [输入Schema](../../preset-plugins/contracts/embedding-multimodal/input.schema.json)。并发、tokens和媒体峰值限制见 [预设](../../preset-plugins/embedding-gemma-multimodal-1/README.md)。

## 输出

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `data` | 是 | 顺序与输入相同 |
| `dimensions` | 是 | 见完整Schema |
| `normalized` | 是 | 见完整Schema |
| `model` | 是 | 见完整Schema |
| `model_revision` | 是 | 构建时锁定的上游 commit |
| `embedding_space_id` | 是 | 不含 CPU/并发数的向量空间指纹 |
| `profile_id` | 是 | 实际插件 ID |
| `usage` | 是 | 本批输入计量，不是模型性能承诺 |

[输出Schema](../../preset-plugins/contracts/embedding-multimodal/output.schema.json)。data顺序与输入相同，向量实际长度必须等于dimensions；所有分量有限，L2范数误差≤1e-4，零向量失败。维度截取发生在归一化之前。

## 权限与失败

必须同时有skill.invoke和目标capability.invoke；多模态还校验被引用Blob读取范围。请求不能覆盖目标插件或输入任意URL。错误沿用 [Skill路由](../routing.md)；不接受模态、维度或超token输入为INVALID_ARGUMENT，未Ready为PROVIDER_UNAVAILABLE，满队列为RESOURCE_EXHAUSTED。

side_effect=none；重试仍受原deadline，不自动更换模型revision/向量空间。[调用样例](../examples/embedding-text.json)供文本profile参考，多模态items须使用parts。

用户批量操作默认使用[异步batch](../batches.md)，每个item.input为上述完整输入；本接口仍是有界执行原语。
