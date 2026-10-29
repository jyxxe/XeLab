# `database.query` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`database.query`；操作目录：`query`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `connection_alias` | `string` | 是 | — | 最短 `1`；最长 `128`；固定批准的连接/路由别名 |
| `sql` | `string` | 是 | — | 最短 `1`；最长 `65536`；单条参数化语句，非shell |
| `parameters` | `array` | 否 | — | 最少项 `0`；最多项 `128` |
| `parameters[].type` | `"null"` / `"boolean"` / `"int64"` / `"decimal"` / `"float64"` / `"text"` / `"bytes"` / `"uuid"` / `"timestamp"` / `"json"` | 父级存在时 | — | — |
| `parameters[].value` | `JsonValue`（引用） | 父级存在时 | — | — |
| `timeout_ms` | `integer` | 否 | `10000` | 最小值 `100`；最大值 `30000` |
| `max_rows` | `integer` | 否 | `1000` | 最小值 `1`；最大值 `10000` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `columns` | `array` | 是 | — | 最少项 `0`；最多项 `256` |
| `columns[].name` | `string` | 父级存在时 | — | 最短 `1`；最长 `128` |
| `columns[].data_type` | `string` | 父级存在时 | — | 最短 `1`；最长 `64` |
| `rows` | `array` | 是 | — | 最少项 `0`；最多项 `10000` |
| `rows[]` | `array` | 随数组 | — | 最少项 `0`；最多项 `256` |
| `row_count` | `integer` | 是 | — | 最小值 `0`；最大值 `10000` |
| `truncated` | `boolean` | 是 | `false` | — |
| `elapsed_ms` | `integer` | 是 | — | 最小值 `0`；最大值 `30000` |

## 业务流

[stream.schema.json](stream.schema.json)。

不提供流式业务输出；`null` 为禁用流的契约标记，调用不得发送业务数据帧。

## 执行约束

[完整执行语义](semantics.json)。

| 项目 | 定义 |
|---|---|
| 副作用分类 | `none` |
| 最大输入 / 输出 | 4194304 / 16777216 字节 |
| JSON 深度 | ≤64 |
| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |
| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |
| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |

补充语义与跨字段约束：

- Read-only DB transaction plus approved schema/table/function policy; no string-built parameters.
- Numbers/int64/decimal/binary follow explicit typed encoding; truncation is explicit.
- Typed SQL values are fixed: bigint/decimal canonical strings, bytea base64, timestamp UTC RFC3339, json I-JSON, null null; reject unsupported types/nonfinite numbers/precision loss; int64 range checked at runtime.

落地门槛：`database-profile-acl`、`sql-policy-parser`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:c5053434f6d048c30c152c6a4dc4052de7bd8191c67add330e2fffac8b138a4a`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
