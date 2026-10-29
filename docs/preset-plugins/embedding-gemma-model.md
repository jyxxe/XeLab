# EmbeddingGemma 2 模型与构建锁

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

本方案的 embedding-gemma 统一使用 `google/embeddinggemma-2`，不是旧 EmbeddingGemma 1、Gemma 2 生成模型或 Gemini Embedding 云接口。

官方模型卡给出文本/图像/音频/视频的共享向量空间，原生768维，可取128/256/512维；本方案按官方要求在截取后重新 L2 归一化。纯文本预设不加载视觉/音频编码器，多模态预设加载对应编码器。[官方模型卡](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2)

CPU默认float32，不使用float16；不自动下载或运行上游自定义代码。后续CPU原生bf16/量化版本必须单独校验并声明新profile/向量空间兼容性。[官方数值精度说明](https://huggingface.co/google/embeddinggemma-2#4-numerical-precision)

## 构建时锁定

镜像 `/xelab/models/embeddinggemma-2.lock.json` 必须包含 model_repository、40位模型revision、tokenizer/processor revision、各权重文件相对路径/字节数/SHA256、推理后端及版本、dtype、前缀策略版本、投影/池化版本。构建后与镜像签名一起发布。

锁文件字段精确定义见 [model-lock.schema.json](model-lock.schema.json)，初始profile固定float32/无量化；不能用示例路径代替实际模型锁。

本仓库只提供预设文档，不捏造上游commit/镜像摘要或下载模型。缺锁、摘要不符、后端不支持所需模态时 Initialize/Ready失败，不能用latest/main临时替代。

## 参数和向量空间

用户选择 purpose：retrieval_query、retrieval_document、semantic_similarity。文本前缀由适配器固定生成，标题仅用于 document，不允许用户修改内部特殊控制序列。媒体按有序parts组装，不把BlobID当文件路径。

embedding_space_id 使用锁定模型/文本投影、processor/tokenizer、前缀策略、dtype/量化、维度及归一化规则计算JCS/SHA256，不包含CPU数、线程数或并发数。两个文本预设在相同锁/参数下应兼容；不同锁或维度不混查。多模态跨模态比较也需同一空间指纹。

模型上下文上限不是每个预设保证支持的输入大小；本预设采用更小token/媒体限额保护给定CPU与内存。具体限制见各目录。所有4GiB/6GiB峰值及8并发吞吐都待实际后端压力验证，超限不得自动借内核内存或加载第二份模型。
