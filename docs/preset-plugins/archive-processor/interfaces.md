# 归档处理接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `archive.extract`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `blob_id` | UUID，必填 | 获授权的核心归档Blob，≤64MiB |
| `format` | `auto/zip/tar`，默认auto | 检测实际格式，不只信任文件名 |
| `max_files` | 1..10000，默认1000 | 成功条目数硬上限 |
| `max_total_bytes` | 1..67108864，默认67108864 | 完整解压字节总上限 |
| `max_depth` | 1..32，默认16 | 归档逻辑路径段数上限 |

成功输出 `files` 数组及 `total_bytes`；每项含 `path/blob_id/size_bytes/digest/content_type`。路径只是归档内逻辑名，导出Blob按当前主体授权；不返回临时目录或宿主路径。空目录不创建可读取Blob，且不承诺保留文件权限/属主。

## `archive.create`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `format` | `zip/tar`，必填 | 目标格式 |
| `entries` | 1..1000项，必填 | 每项 `path/blob_id`，必须全有权读取 |
| `entries[].path` | 安全相对路径 | 不得重复；最多16段 |
| `entries[].blob_id` | UUID | 单项≤64MiB，源内容总量≤64MiB |

成功输出 `blob_id/size_bytes/digest/content_type/file_count`，归档Blob≤64MiB；格式额外开销导致越限也拒绝。目录结构仅从路径推导，不允许输入符号链接、设备节点或宿主文件名。

## 路径与资源边界

所有格式统一拒绝绝对路径、盘符、反斜线、空段、`.`、`..`、NUL、重复规范化路径、符号/硬链接、设备及特殊文件。ZIP/TAR字段声明不可信，必须按实际读取的字节数累计并限制压缩比、条目数、深度与deadline。

输入为嵌套归档时不自动递归解压。超过任何上限返回 `RESOURCE_EXHAUSTED`，不返回一个看似完整的截断文件集。输出Blob只在整项检查完成后发布；失败清理临时文件和未发布Blob引用。

解压/创建不修改持久用户对象，但会发布临时核心Blob，按idempotent处理。结果Blob必须由核心幂等记录复用：同主体、同键、同输入返回原结果，不重复发布Blob；同键异输入返回`STATE_CONFLICT`。发布窗口结果不明时先核实记录，不能盲目重新生成。批量工作使用异步batch，各单项独立原子，不提供跨项事务。

## 原始契约

- `archive.extract`： [输入](contracts/extract/input.schema.json) · [输出](contracts/extract/output.schema.json) · [流](contracts/extract/stream.schema.json) · [语义](contracts/extract/semantics.json)
- `archive.create`： [输入](contracts/create/input.schema.json) · [输出](contracts/create/output.schema.json) · [流](contracts/create/stream.schema.json) · [语义](contracts/create/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

