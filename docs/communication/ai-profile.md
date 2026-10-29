# 文本 AI 兼容子集

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 支持面

只冻结 `/v1/models`、`/v1/chat/completions`、`/v1/embeddings` 三个路径和本规格 Schema 列出的字段。第三方 SDK 的其他字段、工具调用、多模态、logprobs、批量 choice、Responses API 均不在此子集；未知字段返回 INVALID_ARGUMENT，不能静默忽略后宣称全兼容。

模型别名由批准的部署配置映射至明确 ContractRef/提供者，不允许客户端利用别名绕过能力授权。列表只返回当前可见且健康的别名。

## chat/completions

messages 仅 system/user/assistant + 文本 content，n 固定为 1，temperature 默认 1，max_tokens 默认 1024，上限由模型实际约束进一步收窄。超上下文限制返回 400，不静默截断用户输入。

stream=false 返回 ChatResponse；stream=true 返回 SSE ChatChunk。首帧 delta.role=assistant；文本帧 delta.content；结束帧 finish_reason=stop/length；最后 `data: [DONE]`。首帧后失败输出 `event: error`、`data: ErrorEnvelope` 并关闭，不伪造 stop。

同一响应 id/model/created 不变。断线传播取消，不从备用模型无条件重开并混合输出。usage 仅是插件报告并校验的 token 统计，不作为授权事实；total_tokens 必须为前两项之和。

## embeddings

input 为 1–128 个非空字符串，仅 float 输出；data.index 与输入位置一一对应，单模型维度一致且所有数值有限。usage.completion_tokens 固定 0，prompt_tokens 为本批次输入计量。

新embedding-gemma预设必须经模型别名绑定的skill_id调用；核心按其batch上限拆分并保留顺序、根deadline和授权链。该兼容入口仍只接收文本、输出默认768维；知识库默认128维由原生embedding Skill明确传dimensions=128，不改写本兼容子集。

## 错误与演进

本平台当前使用统一 ErrorEnvelope，不能承诺第三方错误包装完全相同。兼容 profile 独立版本管理；未来增强字段必须附适配与取消/计量测试，不能改变旧字段含义。
