# `archive.extract` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`archive.extract`；操作目录：`extract`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `blob_id` | `string` | 是 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `format` | `"auto"` / `"zip"` / `"tar"` | 否 | `"auto"` | — |
| `max_files` | `integer` | 否 | `1000` | 最小值 `1`；最大值 `10000` |
| `max_total_bytes` | `integer` | 否 | `67108864` | 最小值 `1`；最大值 `67108864` |
| `max_depth` | `integer` | 否 | `16` | 最小值 `1`；最大值 `32` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `files` | `array` | 是 | — | 最少项 `0`；最多项 `10000` |
| `files[].path` | `string` | 父级存在时 | — | 最短 `1`；最长 `1024`；相对规范路径，无dot/链接/设备节点 |
| `files[].blob_id` | `string` | 父级存在时 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `files[].size_bytes` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `67108864` |
| `files[].digest` | `string` | 父级存在时 | — | 最短 `1`；最长 `71`；模式 `"^sha256:[a-f0-9]{64}$"`；SHA256 |
| `files[].content_type` | `string` | 父级存在时 | — | 最短 `1`；最长 `128` |
| `total_bytes` | `integer` | 是 | — | 最小值 `0`；最大值 `67108864` |

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

- All extracted files are temporary authorized Blob references; total<=64MiB.
- Reject symlinks/hardlinks/traversal/absolute names/encrypted unsupported formats and archive bombs.
- Output Blob publication is part of a durable per-Call idempotency ledger; replay reuses existing result/Blob IDs while authorized, never duplicate publication.

落地门槛：`archive-libraries`、`bounded-multi-blob-output`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:07aa7debcec55535b41bdec7efaa118eef90ee07446381fdb29d594ca018e955`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
