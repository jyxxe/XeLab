# events 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| POST | `/api/platform/v1/events` | [发布持久化事件](publishEvent.md) |
| GET | `/api/platform/v1/subscriptions` | [列出事件订阅](listSubscriptions.md) |
| POST | `/api/platform/v1/subscriptions` | [创建拉取式订阅](createSubscription.md) |
| GET | `/api/platform/v1/subscriptions/{subscription_id}` | [读取订阅](getSubscription.md) |
| DELETE | `/api/platform/v1/subscriptions/{subscription_id}` | [禁用订阅](deleteSubscription.md) |
| GET | `/api/platform/v1/subscriptions/{subscription_id}/deliveries` | [SSE 拉取租约内事件](streamDeliveries.md) |
| POST | `/api/platform/v1/subscriptions/{subscription_id}/acks` | [确认或重试事件投递](ackDelivery.md) |
