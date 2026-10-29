# 通知编排接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `notification.send`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `route_alias` | string，必填 | 预先批准的email或Webhook渠道路由，不能传任意URL或Skill目标 |
| `recipients` | 1..100个字符串，必填 | 渠道的目标标识；按路由校验地址或订阅目标 |
| `message.subject` | string，可省略 | 邮件主题或通知标题，禁止CR/LF注入 |
| `message.text` | 非空string，必填 | 通知文本 |
| `message.html` | string，可省略 | 仅批准支持HTML的邮件路由可用 |
| `dedupe_key` | string，必填 | 主体与路由内唯一的业务去重键，须与核心调用幂等键关联 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `notification_id` | UUID | 持久通知记录 |
| `status` | `accepted/delivered/failed/unknown` | accepted为上游明确接收；delivered须有上游同步终态证据；否则不宣称送达 |
| `channel` | `email/webhook` | 实际选定的批准渠道 |
| `attempts` | 非负integer | 已完成或结果不明的物理投递次数 |
| `provider_message_id` | string或null | 提供者给出的有界非秘密标识 |

## 编排、权限与状态

路由由管理员固定映射到具体安装/Skill及目标策略。调用链同时满足notification能力和子调用的Skill、底层能力、Secret与网络授权；不能以插件自身身份绕过用户权限或根预算。

先持久化去重记录，再提交投递；相同主体/路由/键且同输入返回原记录，异输入返回 `STATE_CONFLICT`。限流按主体、渠道和提供者分别实施；重试仅在根deadline内使用有界退避并保留调用来源。

仅能证明未产生副作用的失败，或提供者明确支持相同幂等键的重放，才允许自动重试。超时但可能已接收必须为unknown，暂停后续渠道fallback，避免重复通知。邮件接受、WebHook返回成功与用户最终阅读不是同一状态。

v0.1.0在根deadline内完成一次有界投递并返回上游明确结果或unknown；仅本地排队不能返回accepted。Call完成后结果固定，不承诺后台从accepted继续变成delivered；上游仅受理时保留accepted，不将受理推断为送达。

大量发送由核心异步batch排队，逐项记录结果。真正后台通知队列、投递查询或回调需要单独的gate/API与授权续期机制；本版不提供此类路径，不能持有旧用户token无限后台运行，也不能把整批标为全局送达。

## 原始契约

- `notification.send`： [输入](contracts/send/input.schema.json) · [输出](contracts/send/output.schema.json) · [流](contracts/send/stream.schema.json) · [语义](contracts/send/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

