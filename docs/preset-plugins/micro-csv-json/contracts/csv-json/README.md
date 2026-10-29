# `convert.csv-json` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`convert.csv-json`；操作目录：`csv-json`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `csv` | `string` | 是 | — | 最短 `0`；最长 `1048576` |
| `delimiter` | `","` / `";"` / `"\t"` | 否 | `","` | — |
| `header` | `boolean` | 否 | `true` | — |
| `columns` | `array` | 否 | — | 最少项 `1`；最多项 `128`；元素唯一 `true` |
| `columns[]` | `string` | 随数组 | — | 最短 `1`；最长 `256` |
| `null_value` | `string` | 否 | — | 最短 `0`；最长 `256` |
| `max_rows` | `integer` | 否 | `1000` | 最小值 `1`；最大值 `10000` |
| `max_columns` | `integer` | 否 | `128` | 最小值 `1`；最大值 `128` |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `columns` | `array` | 是 | — | 最少项 `1`；最多项 `128`；元素唯一 `true` |
| `columns[]` | `string` | 随数组 | — | 最短 `1`；最长 `256` |
| `rows` | `array` | 是 | — | 最少项 `0`；最多项 `10000` |
| `rows[]` | `object` | 随数组 | — | 最多成员 `128` |
| `row_count` | `integer` | 是 | — | 最小值 `0`；最大值 `10000` |

## 业务流

[stream.schema.json](stream.schema.json)。

不提供流式业务输出；`null` 为禁用流的契约标记，调用不得发送业务数据帧。

## 执行约束

[完整执行语义](semantics.json)。

| 项目 | 定义 |
|---|---|
| 副作用分类 | `none` |
| 最大输入 / 输出 | 1048576 / 1048576 字节 |
| JSON 深度 | ≤64 |
| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |
| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |
| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |

补充语义与跨字段约束：

- RFC4180 quoting/newlines, duplicate headers reject, explicit null_value only.
- No formula evaluation or implicit numeric coercion; limits are errors, not silent truncation.

落地门槛：`csv-rfc4180-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:ccc307556bd67f5499df874fd1137f09d7ed076ef66181ee9338d79d32b7c9b0`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
