# system 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/health/live` | [存活探针](getLiveness.md) |
| GET | `/health/ready` | [就绪探针](getReadiness.md) |
| GET | `/api/platform/v1/node` | [读取本机预算和资源状态](getNode.md) |
| GET | `/api/platform/v1/metrics` | [读取 Prometheus 文本指标](getMetrics.md) |
| PUT | `/api/platform/v1/node/config` | [更新本机资源预留配置](updateNodeConfig.md) |
| GET | `/api/platform/v1/operations/{operation_id}` | [查询异步操作](getOperation.md) |
| POST | `/api/platform/v1/operations/{operation_id}/cancel` | [取消异步操作](cancelOperation.md) |
| GET | `/api/platform/v1/audit-events` | [查询脱敏审计](listAuditEvents.md) |
