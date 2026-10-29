# CLI 命令与恢复接口

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 通用 API 命令

```text
xelabctl [--endpoint URL | --socket PATH] [--token-file PATH]
         [--output json|table] api OPERATION_ID
         [--path NAME=VALUE ...] [--query NAME=VALUE ...]
         [--body-file PATH|-] [--if-match '"rev-N"']
         [--idempotency-key KEY] [--timeout-ms N]
```

OPERATION_ID 必须来自 [HTTP 目录](../api/README.md)，参数名与 Schema 一致；没有默认猜测路径或自动扩大权限。非只读操作自动生成幂等 UUID 并输出安全恢复标识；调用者可显式提供原键重试。一次性凭证只在首次响应写 stdout，不写普通日志。

token-file 必须为仅本人可读文件；body-file=- 从 stdin 读取，密码和 secret 不放命令行参数。TLS 默认验证；socket 来源也需映射到经批准宿主身份或用户凭证，不能对任意 localhost 进程放行。

## 宿主专用命令

| 命令 | 参数 | 输入/输出及权限 |
|---|---|---|
| xelabctl bootstrap | --config PATH --owner-name NAME | 初次未初始化时；宿主受保护身份，stdin 读取密码；返回 Owner ID，不返回密码 |
| xelabctl recover-owner | --config PATH --account-id UUID | 需要受保护宿主身份及领导锁；重设激活路径/独立恢复根，撤销旧登录会话，记录审计 |
| xelabctl migrate | --config PATH --target INTEGER | 校验备份和迁移 checksum，显式目标；不自动执行破坏性 downgrade |
| xelabctl validate-config | --config PATH | 校验 Schema、权限、预算/隔离前提；返回脱敏诊断，不启动插件 |
| xelabctl reconcile | --config PATH | 仅连接已认证本机管理入口唤醒控制循环；不直接修改 OCI/数据库 |

bootstrap 只成功一次；recover-owner 不能与活动迁移/恢复事务并发绕过锁。普通管理员启停 WebUI 使用 `api setDeploymentEnabled`，不要求修改宿主服务。

## 退出码

0 成功；2 参数/Schema 错误；3 未认证；4 无权；5 状态/版本冲突；6 超时或结果未知；7 资源/服务不可用；8 内部错误。收到 202 默认输出 Operation 不等待；需要轮询由 `api getOperation` 明确执行。

CLI 规范为待实现接口；仓库当前没有可执行 xelabctl 二进制。
