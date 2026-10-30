# 幂等、超时与副作用去重

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## HTTP 管理幂等

作用域为 `(subject_id, credential_id, method, normalized_path, Idempotency-Key)`。规范请求指纹为请求体的规范 JSON SHA-256、重要条件头和查询参数；只包含本次行为相关字段。JSON 规范化采用 [RFC 8785 JCS](https://www.rfc-editor.org/rfc/rfc8785.html)，原始二进制用内容摘要。路径按已匹配路由及规范化标识表示，查询参数按名称和值排序；不能混淆百分号编码别名。

幂等记录与首个业务变更同事务提交，保留 24h。相同键不同指纹为 409；正在执行返回原 Operation；成功返回原结果但仍重新校验当前授权，已无权不可读取旧结果。一次性响应只保留资源引用，不缓存明文。

fingerprint 包含首次 If-Match，但重试同键先查原幂等结果再执行当前 ETag 检查；不能因为首次操作已更新 revision 就让完全相同的安全重放失败。新键则必须检查当前 ETag。

## 业务能力去重

能力 side_effect=none 可在未开始流式输出前按策略安全重试；idempotent 要求插件跨重启保存稳定键→结果；queryable 支持 QueryResult 核实；non_repeatable 中断进入 unknown。

能力幂等键绑定主体源链、契约 name/version/digest、提供者数据域与输入摘要。同一步骤跨尝试保留同一业务键；新 attempt_id/fence 只表示执行尝试，不应绕过去重。

## 重试责任

网关、RPC SDK、TaskEngine 不能各自默认重试造成乘法放大。TaskEngine 为任务唯一重试决策者；同步 Broker 最多总计 2 次且仅 none/已验证 idempotent、未提交响应、整体 deadline 内执行。非幂等状态不明绝不自动切换提供者。

退避使用指数增长加 0–20% 抖动，尊重 Retry-After 和最大尝试数；deadline 包含排队、连接及执行时间，不能每次重试重新计时。
