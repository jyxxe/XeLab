# 日志、指标、追踪与审计

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 日志与追踪

结构化日志包含 at、level、plugin_id/deployment_id/instance_id、request_id、task_id、operation_id 和稳定 reason_code；HTTP LogEntry 是受权限控制的投影。用户及 credential ID 仅在受保护审计/日志中使用，不作为高基数指标标签。

禁止记录 Authorization、Cookie、密码、激活码、API Key 明文、secret、完整原始业务输入输出。外部错误先脱敏，日志行最大 8KiB，超长截断并记录 truncated=true 的内部标记。

## 指标接口

`GET /api/platform/v1/metrics`，platform.read，Prometheus 0.0.4 文本，Content-Type text/plain。指标有如下固定名称、类型与单位：

| 名称 | 类型 | 标签及单位 |
|---|---|---|
| xelab_http_requests_total | counter | method、operation_id、status_class；次数 |
| xelab_http_duration_seconds | histogram | operation_id；秒；桶 .01/.05/.1/.5/1/5/10/30/300 |
| xelab_plugin_cpu_usage_seconds_total | counter | plugin_id；累计 CPU 秒 |
| xelab_plugin_memory_bytes | gauge | plugin_id；当前字节 |
| xelab_plugin_reserved_cpu_millicores | gauge | plugin_id；获批 CPU |
| xelab_plugin_reserved_memory_bytes | gauge | plugin_id；获批内存 |
| xelab_tasks | gauge | state；当前任务数量 |
| xelab_authz_decisions_total | counter | outcome、reason_code；次数 |
| xelab_outbox_pending | gauge | topic；待发送条数 |
| xelab_volume_used_bytes | gauge | plugin_id；已用容量聚合 |

plugin_id 标签数量受安装配额限制，不能带用户提交的任意字符串。trace 传播内核生成 request_id，插件只能提供附加关联，不能伪造审计主体。

## 告警与审计

至少告警：内核/宿主余量不足、OOM、连续重启、卷满、控制 DB 不可用、outbox 超 60s 未投递、任务租约频繁过期、凭证异常使用。告警阈值属于部署配置，不能当作接口成功保证。

审计覆盖授权拒绝和敏感成功操作；记录实际主体、执行实例、源链、资源、动作及策略版本，追加保存。普通日志读取与 audit.read 分离；审计员默认不读取原业务内容。
