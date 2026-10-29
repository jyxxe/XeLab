# `git.repository` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`git.repository`；操作目录：`repository`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `repository_alias` | `string` | 是 | — | 最短 `1`；最长 `128`；固定批准的连接/路由别名 |
| `operation` | `"files"` / `"branches"` / `"log"` | 是 | — | — |
| `path` | `string` | 否 | — | 最短 `0`；最长 `1024` |
| `ref` | `string` | 否 | — | 最短 `1`；最长 `256` |
| `limit` | `integer` | 否 | `50` | 最小值 `1`；最大值 `200` |
| `cursor` | `string` | 否 | — | 最短 `1`；最长 `4096` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `operation` | `"files"` / `"branches"` / `"log"` | 是 | — | — |
| `resolved_commit` | `string` 或 `null` | 是 | — | — |
| `files` | `array` | 是 | — | 最少项 `0`；最多项 `200` |
| `files[].path` | `string` | 父级存在时 | — | 最短 `1`；最长 `1024` |
| `files[].kind` | `"file"` / `"directory"` | 父级存在时 | — | — |
| `files[].size_bytes` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `67108864` |
| `files[].blob_id` | `string` 或 `null` | 父级存在时 | — | — |
| `branches` | `array` | 是 | — | 最少项 `0`；最多项 `200` |
| `branches[].name` | `string` | 父级存在时 | — | 最短 `1`；最长 `256` |
| `branches[].commit` | `string` | 父级存在时 | — | 最短 `1`；最长 `64` |
| `commits` | `array` | 是 | — | 最少项 `0`；最多项 `200` |
| `commits[].id` | `string` | 父级存在时 | — | 最短 `1`；最长 `64` |
| `commits[].parents` | `array` | 父级存在时 | — | 最少项 `0`；最多项 `32` |
| `commits[].parents[]` | `string` | 随数组 | — | 最短 `1`；最长 `64` |
| `commits[].author_name` | `string` | 父级存在时 | — | 最短 `0`；最长 `256` |
| `commits[].authored_at` | `string` | 父级存在时 | — | 最短 `1`；最长 `40`；格式 `"date-time"` |
| `commits[].subject` | `string` | 父级存在时 | — | 最短 `0`；最长 `4096` |
| `next_cursor` | `string` 或 `null` | 是 | — | — |

## 业务流

[stream.schema.json](stream.schema.json)。

不提供流式业务输出；`null` 为禁用流的契约标记，调用不得发送业务数据帧。

## 执行约束

[完整执行语义](semantics.json)。

| 项目 | 定义 |
|---|---|
| 副作用分类 | `idempotent` |
| 最大输入 / 输出 | 4194304 / 16777216 字节 |
| JSON 深度 | ≤64 |
| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |
| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |
| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |

补充语义与跨字段约束：

- Read-only; no push/reset/hooks/submodule execution or arbitrary remote URL.
- Resolve symbolic ref once and bind cursor to commit.
- Output Blob publication is part of a durable per-Call idempotency ledger; replay reuses existing result/Blob IDs while authorized, never duplicate publication.

落地门槛：`git-transport-policy`、`repository-profile-acl`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:289021e7cf513918420d1985212d5a53a47100261d9342a4ef847ea4c19dbf3a`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
