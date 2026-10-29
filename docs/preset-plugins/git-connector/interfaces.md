# Git 仓库连接器接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `git.repository`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `repository_alias` | string，必填 | 批准的仓库地址、凭证及只读范围 |
| `operation` | `files/branches/log`，必填 | 只读子操作 |
| `ref` | string，可省略 | files/log用；省略为批准默认分支，解析后固定commit |
| `path` | string，可省略 | files的文件或目录路径，log的可选路径过滤；安全相对路径 |
| `limit` | 1..200，默认50 | 返回条目数 |
| `cursor` | string，可省略 | 同主体/仓库/操作/快照绑定的不透明游标 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `operation` | 枚举 | 回显实际子操作 |
| `resolved_commit` | string或null | files/log固定的完整提交ID；branches可为null |
| `files` | array | files用；每项 `path/kind/size_bytes/blob_id`，目录blob_id为null |
| `branches` | array | branches用；每项 `name/commit` |
| `commits` | array | log用；每项 `id/parents/author_name/authored_at/subject` |
| `next_cursor` | string或null | 未完成时的同快照续页标识 |

只填当前操作对应的数组，其余为空。files输入目录时列目录项；输入普通文件时导出核心Blob，≤64MiB；不能把源文件字节塞入JSON。kind仅为file/directory，拒绝跟随或导出符号链接，未支持对象类型不能伪装成普通文件。提交作者字段只是仓库声明，不表示平台认证身份。

能力因文件导出按idempotent处理，结果Blob必须由核心幂等记录复用；同主体、同键、同输入返回既有结果与Blob，不重复发布。同键异输入返回`STATE_CONFLICT`；导出窗口结果不明先核实，不能盲目重发。分支和日志读取不产生额外业务写入。

## 只读与隔离

不接收任意仓库URL、远程协议、命令行参数或凭证。Git配置禁用hook、过滤器和任意外部命令；拒绝file/本地路径及未经批准的transport。私有仓库认证、重定向及子请求仍受网络策略限制。

路径拒绝绝对路径、`..`、NUL与宿主路径逃逸；ref必须经Git安全解析且不作为shell文本。限制克隆/fetch字节、对象数、单对象、历史步数和临时盘。游标失效返回 `STATE_CONFLICT`，不能悄悄换成更新的HEAD继续。

缓存按仓库权限边界隔离；每次调用重新验证主体与当前授权，撤销权限不能通过旧游标或缓存读到私有仓库。批处理仍使用核心异步batch。

## 原始契约

- `git.repository`： [输入](contracts/repository/input.schema.json) · [输出](contracts/repository/output.schema.json) · [流](contracts/repository/stream.schema.json) · [语义](contracts/repository/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

