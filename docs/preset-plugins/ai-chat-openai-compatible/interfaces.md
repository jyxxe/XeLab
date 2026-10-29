# OpenAI 兼容聊天接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `ai.chat`

文本业务输入复用 [ChatRequest](../../contracts/schemas/ChatRequest.schema.json)，非流式输出复用 [ChatResponse](../../contracts/schemas/ChatResponse.schema.json)，文本增量复用 [ChatChunk](../../contracts/schemas/ChatChunk.schema.json)。

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `model` | 字符串，必填，1..128字符 | 已批准的模型别名；不能传任意提供者 URL |
| `messages` | 1..256项，必填 | 每项仅 `role=system/user/assistant` 与文本 `content` |
| `stream` | boolean，默认false | 选择普通响应或流式响应 |
| `temperature` | number，0..2，默认1 | 受具体模型能力进一步收窄 |
| `max_tokens` | integer，1..32768，默认1024 | 生成上限，不能突破模型上下文限制 |

普通输出为 `id/object/created/model/choices/usage`；`choices` 恰好1项，包含文本 assistant 消息和 `finish_reason=stop/length`。usage含 `prompt_tokens/completion_tokens/total_tokens`，总数必须等于前两者之和；统计只表示提供者报告，不能作为授权依据。

流式输出保持同一 `id/model/created`，每块恰好1个choice；先输出 `delta.role=assistant`，再输出 `delta.content`，最后输出 `finish_reason`。平台兼容入口按既有profile转换为SSE和终止标记；原生Skill传输使用核心业务流封装，不能另开绕过鉴权的SSE地址。

## 路由与失败

模型别名固定绑定批准的提供者配置与凭证，不接收裸密钥。首块发出后不得将其他模型的生成内容拼入同一流；断线或取消应向外部提供者传播。流中故障走核心错误事件并关闭，不伪造成功终止。

未输出前是否重试必须受总deadline、提供者计费语义和幂等能力约束；结果不明时停止自动重试。超上下文返回 `INVALID_ARGUMENT`，不静默截断；限流、资源不足、权限和超时使用核心统一错误。

## 原始契约

- `ai.chat`： [输入](contracts/chat/input.schema.json) · [输出](contracts/chat/output.schema.json) · [流](contracts/chat/stream.schema.json) · [语义](contracts/chat/semantics.json)

仅`ai.chat`提供上述文本增量流，batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

