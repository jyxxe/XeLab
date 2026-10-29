# 邮件发送连接器接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `email.send`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `transport_alias` | string，必填 | 已批准的SMTP/API配置与发件人；不能输入任意DSN |
| `to` | 1..100个地址，必填 | 主收件人 |
| `cc`、`bcc` | 各至多100个地址，默认[] | to+cc+bcc总收件人数≤100，再受部署额度收窄 |
| `subject` | string，必填 | 不能含CR/LF或注入邮件头 |
| `text`、`html` | string，可省略 | 直接正文模式至少一个；与模板模式互斥 |
| `template_alias`、`template_data` | string / JSON对象 | 模板模式；别名绑定固定批准版本，不执行任意代码 |
| `attachments` | 最多10项，默认[] | 每项 `blob_id/filename/content_type`；只读可授权Blob |
| `dedupe_key` | string，必填 | 当前主体/transport内的业务去重键 |

单附件与合计原始附件均≤64MiB，并受部署或提供者更小额度收窄；邮件编码膨胀也计入提供者限制。附件名必须是安全显示名，不能作为宿主路径。模板渲染和内容生成后仍须检查总大小。

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `message_id` | UUID | 本地持久邮件记录 |
| `status` | `accepted/sent/failed/unknown` | sent仅表示传输端已接受，不等于送达或阅读 |
| `provider_message_id` | string或null | 有界的提供者记录ID |
| `accepted_recipients` | 地址数组 | 已明确被传输端接受的目标 |
| `rejected_recipients` | 地址数组 | 已明确拒绝的目标，不含秘密错误原文 |

## 权限、幂等与部分接受

发件人、认证机制、SMTP/API地址、TLS和网络策略由transport配置固定；不接收裸Secret或任意邮件头。邮件目标与域名可按角色/权限组进一步收窄。Bcc不能泄露给其他收件人，审计按权限脱敏正文与附件元数据。

同去重键同输入返回原记录，异输入返回 `STATE_CONFLICT`。提供者部分接受或连接断开不能把整封邮件视为无副作用失败；默认停止自动重试，避免对已接受目标重复发送。只有目标端去重/查询能够证明安全时才允许恢复。

核心取消只停止后续工作，不撤销已被SMTP/API接受的邮件。失败处理保留已知接受情况；不得把API成功响应直接宣称为最终投递成功。

## 原始契约

- `email.send`： [输入](contracts/send/input.schema.json) · [输出](contracts/send/output.schema.json) · [流](contracts/send/stream.schema.json) · [语义](contracts/send/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

