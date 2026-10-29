# Webhook 触发器接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## 入口边界

状态为 `planned`，公有HTTP入口gate为 `blocked`。现有核心认证路径和保留命名空间不能据此被插件占用；也不能绕过核心直接暴露OCI容器端口。

待冻结的入口适配负责按原始请求字节验证签名、时间窗口、重放ID、内容类型、大小和来源策略，再生成可信主体及以下规范化业务输入。签名算法、HTTP路径、挑战验证和提供商响应码不在本轮伪造为已支持。

事件模式可复用HostService.PublishEvent，仍受`trusted-webhook-ingress` gate和事件发布授权约束。任务模式的`host-task-submission` gate为blocked：HostService尚无CreateTask，需先冻结带可信来源、授权与幂等语义的受控任务提交接口；不得改走用户HTTP路径规避。

## `trigger.webhook`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `authenticated_event_id` | UUID，必填 | 未来核心认证入口签发的事件记录ID；必须在可信上下文中核实 |
| `source_alias` | string，必填 | 与该认证记录、endpoint及签名来源绑定，不能由别名自行证明身份 |
| `endpoint_alias` | string，必填 | 已批准的入口、凭证和事件/任务映射 |
| `event_id` | string，必填 | 认证来源提供的事件ID，用于持久去重 |
| `event_type` | string，必填 | 路由允许的事件类型，不是任意核心事件名 |
| `payload` | JSON值，必填 | 入口规范化并校验的有界业务体，不含裸签名秘密 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `accepted` | boolean | 已持久接收；不等于任务已执行 |
| `duplicate` | boolean | 相同来源/endpoint/event_id的既有输入 |
| `receipt_id` | UUID | 持久接收与审计记录 |
| `task_id` | UUID或null | 成功创建的持久任务 |
| `platform_event_id` | string或null | 成功发布的平台事件标识，与来源event_id区分 |

可信来源、签名通过状态和接收时间必须来自核心入口上下文，不能允许调用者在payload中伪造。原始JSON中的authenticated_event_id和source_alias也不能独自证明认证；核心须核实可信入口记录和来源绑定。普通Skill调用此能力仍须满足同等上下文，不能通过传任意ID/别名冒充外部认证。

## 一致性与重放

先持久化来源去重键与输入摘要，再进行事件/任务提交。同键同输入返回既有receipt；同键不同内容返回 `STATE_CONFLICT`。接收记录和核心事件/任务之间需要可恢复的outbox或等价协议，崩溃后不能无凭据重复触发。

仅返回已持久化的accepted；远端调用结果不明时保持unknown并由恢复流程核实。限流、认证与重放拒绝必须发生在创建业务副作用之前。Webhook高频流入采用队列和任务，不能将原始HTTP连接当作无限执行时长。

## 原始契约

- `trigger.webhook`： [输入](contracts/webhook/input.schema.json) · [输出](contracts/webhook/output.schema.json) · [流](contracts/webhook/stream.schema.json) · [语义](contracts/webhook/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

