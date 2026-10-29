# 对象存储接口与对象模型

[对象存储目录](README.md) · [Skill目录](../../skills/object-storage/README.md) · [存储与一致性](storage.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

所有业务使用`POST /api/platform/v1/skills/{skill_id}/calls`；skill_id为固定版本注册所得UUID，结果位于`SkillCallResult.call.output`。以下接口不表示另开未认证的文件HTTP路径。

## 业务操作

Skill完整名称统一为`org.xelab.object-storage.<后缀>@1.0.0`；目标能力均为1.0.0，映射identity，无业务结果流。

| 后缀 | 目标能力 | 输入概要 | 输出 | 副作用 |
|---|---|---|---|---|
| [object-put](../../skills/object-storage/object-put.md) | `data.object.put` | key、blob_id、size_bytes、digest、content_type、mode、条件expected_revision | `{object}` | idempotent |
| [object-list](../../skills/object-storage/object-list.md) | `data.object.list` | prefix、limit、cursor均可省略 | `{items,next_cursor}` | none |
| [object-stat](../../skills/object-storage/object-stat.md) | `data.object.stat` | key | `{object}` | none |
| [object-export](../../skills/object-storage/object-export.md) | `data.object.export` | key、expected_revision | `{object_id,revision,blob_id,size_bytes,digest,content_type}` | idempotent |
| [object-delete](../../skills/object-storage/object-delete.md) | `data.object.delete` | key、expected_revision | `{object_id,deleted_revision}` | idempotent |
| [usage-get](../../skills/object-storage/usage-get.md) | `data.object.usage.get` | `{}` | 本用户容量与额度 | none |

每项输入均拒绝未知字段，尤其是owner_id、路径、任意URL和原始字节。原始Schema位于`contracts/<后缀>/input.schema.json`及`output.schema.json`，各Skill页面提供直接链接。

## 对象元数据

| 字段 | 类型 | 定义 |
|---|---|---|
| `id` | UUID字符串 | 对同一未删除逻辑对象稳定；删除后同key新建获得新ID |
| `owner_id` | UUID字符串 | 内核可信subject，不能由业务输入覆盖 |
| `key` | 字符串 | 用户命名空间中的完整逻辑key |
| `revision` | 正整数 | 当前可见版本；同一(owner,key)的首次创建从1开始，覆盖、删除和重建均推进永久head水位，不因重启或重建回退 |
| `size_bytes` | 整数 | 0..67108864，实际内容字节数 |
| `digest` | 字符串 | `sha256:`加64位小写十六进制，实际完整内容摘要 |
| `content_type` | 字符串 | 已验证格式的声明MIME，最多128字符，不含CR/LF |
| `created_at` | UTC date-time字符串 | 首次成功创建时刻，覆盖不改变 |
| `updated_at` | UTC date-time字符串 | 最近成功覆盖时刻，不作为CAS凭证 |

metadata不含物理文件路径、原BlobID、历史版本内容或其他用户占用。revision表示当前对象版本；本版不提供历史版本列表、恢复或旧revision下载接口。

delete也消耗一个revision，返回推进后的deleted_revision；同key重建分配新ID并继续推进该key的水位。保留不可作为下载对象的head/tombstone，避免旧expected_revision在删除重建后误匹配新对象。

## key与前缀

key必须为NFC规范化UTF-8，长度1..1024字节；JSON Schema字符数检查之外仍需按编码字节校验。禁止控制字符、反斜线、绝对路径形式、开头/末尾`/`、空段和独立`.`或`..`段；不静默折叠或重写不合法key。

`reports/2026/a.txt`中的`/`仅表示逻辑前缀，不创建可寻址宿主目录。JSON字符串不执行URL解码；`%2F`等字面文本不变成路径分隔符。存储实现不得把key拼接为文件系统路径。

list.prefix是UTF-8字节前缀，可为空或以`/`结尾，例如`reports/`；它仍须NFC、最多1024字节且不含控制字符/反斜线，不要求是一个完整key。匹配区分大小写，不做locale排序、大小写折叠或URL解码。

## 前置条件与错误

所有接口先验证当前用户、Skill及原目标能力权限，再进入用户命名空间。缺失对象和其他用户的同key均不能用于枚举跨用户资源。

| 错误码 | 触发情况 | 处理原则 |
|---|---|---|
| `INVALID_ARGUMENT` | key、字段组合、长度、摘要、MIME或游标格式不合法；实际源内容与声明不符 | 不发布对象，不盲目重试 |
| `PERMISSION_DENIED` | 当前来源、插件批准、Skill/能力或Blob读取权限不足 | 不切换身份绕过 |
| `RESOURCE_NOT_FOUND` | 当前用户对象不存在或所需Blob已不可用 | 重新核实对象/临时Blob状态 |
| `STATE_CONFLICT` | create遇到同key、revision失配、同幂等键不同输入、失效/不匹配游标 | 使用新读取结果决策 |
| `RESOURCE_EXHAUSTED` | 用户额度、部署预算、卷容量或运行资源不足 | 未提交写入不得可见；待资源恢复 |
| `DEADLINE_EXCEEDED` | 原总期限耗尽 | 写操作用原键/Call核实，不能据此推断未提交 |
| `INTERNAL` | 数据损坏、持久化失败等服务内部故障 | 保留可恢复状态，不发布半成品 |

批处理使用[异步batch](../../skills/batches.md)，逐项保留上述输入、授权、CAS和结果；不提供跨项原子性。纯读接口不修改业务对象或配额，核心审计与限流仍然有效。
