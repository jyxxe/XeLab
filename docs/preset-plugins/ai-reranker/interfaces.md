# 重排序接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `ai.rerank`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `model_alias` | string，必填 | 已批准且固定版本的重排序模型配置 |
| `query` | 非空string，必填 | 查询文本 |
| `documents` | 1..1000项，必填 | 每项 `id/text`，id在本请求内唯一，text为候选正文 |
| `top_k` | 1..1000，可省略 | 不得超过documents数量；省略返回全部候选 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `model` | string | 实际固定模型别名/版本标识 |
| `results` | array | 按相关性降序排列；指定top_k时恰好top_k项，否则为全部候选 |
| `results[].id` | string | 来自输入候选id，不能生成候选之外的ID |
| `results[].index` | integer | 原documents数组下标，从0开始 |
| `results[].score` | 有限number | 模型原始分数，不宣称归一化概率 |
| `usage` | object或null | 已校验的 `input_tokens`，提供者未提供时为null |

同分按输入index升序，保证给定分数下结果确定。分数含义绑定模型版本，不跨模型归一化；不能将重排序分数当作知识库向量距离。正文不回显，结果通过id/index关联原记录。

## 长度、授权与失败

输入还受模型查询、文档和总token预算收窄；越界返回 `INVALID_ARGUMENT`，不能静默截断后宣称同一语义。远程部署需要批准候选内容发送范围，Secret仅通过核心绑定；本地部署须在独立CPU/内存配额内加载固定模型。

提供者超时或限流在未输出结果时按根deadline和费用语义处理；不跨模型静默fallback后混合分数。批量重排序通过异步batch逐项执行；单项候选集不会被隐式拆为互不比较的若干组。

## 原始契约

- `ai.rerank`： [输入](contracts/rerank/input.schema.json) · [输出](contracts/rerank/output.schema.json) · [流](contracts/rerank/stream.schema.json) · [语义](contracts/rerank/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

