# 事件投递、确认与死信

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 发布

事件为 topic/version/payload/dedupe_key；主题契约须来自已验证插件能力包。发布者只能发布批准命名空间及目标范围。去重键作用域 `(publisher_id,topic,version,dedupe_key)`，24h 内同键不同内容返回冲突。

业务事务与事件 outbox 同事务提交；投递器随后把 outbox 变为 Event。仅 API 返回成功不意味着任何订阅者已经处理。

## 拉取与租约

每个 Subscription 有独立游标，start=latest 从创建水位之后，earliest 从仍在保留期的首个事件开始。活跃订阅每次只有一个未确认 delivery，租约 30s；断线不会确认。

ack 校验订阅权限、当前 delivery_id 与 lease_token。重复确认已 ack 同一租约视为成功；过期 lease 返回 409，不允许旧消费者推进新租约游标。

retry 按 1/2/4/8/16s（加抖动，最大 60s）退避；超过 maximum_attempts 进入死信，再推进游标。dead_letter 明确终止本消息并记录原因。消费者须按 event_id/自身副作用键去重，平台只承诺至少一次投递。

## 保留和权限

事件默认保留 7 天、死信 30 天，活跃未确认事件保留至重试终态但仍受总存储配额；满额拒绝新发布并告警，不能静默丢弃。订阅停用后拒绝后续投递及确认，元数据保留审计。

本版不支持向任意外部 webhook URL 推送，避免引入未定义的网络权限与重放边界。插件消费使用 HostService.Subscribe/AckEvent，外部客户端使用 SSE/ack。
