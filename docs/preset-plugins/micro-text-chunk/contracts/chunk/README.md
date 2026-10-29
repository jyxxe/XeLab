# `text.chunk` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`text.chunk`；操作目录：`chunk`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `text` | `string` | 是 | — | 最短 `0`；最长 `1048576` |
| `mode` | `"characters"` / `"paragraphs"` / `"tokens"` / `"tokens-estimate"` | 否 | `"characters"` | — |
| `size` | `integer` | 是 | — | 最小值 `1`；最大值 `1048576` |
| `overlap` | `integer` | 否 | `0` | 最小值 `0`；最大值 `1048575` |
| `tokenizer_id` | `string` | 否 | — | 最短 `1`；最长 `128` |
| `tokenizer_version` | `string` | 否 | — | 最短 `1`；最长 `128` |
| `estimator_id` | `"utf8-div4-v1"` | 否 | `"utf8-div4-v1"` | — |
| `estimator_version` | `"0.1.0"` | 否 | `"0.1.0"` | — |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `chunks` | `array` | 是 | — | 最少项 `0`；最多项 `10000` |
| `chunks[].text` | `string` | 父级存在时 | — | 最短 `0`；最长 `1048576` |
| `chunks[].start` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `9007199254740991` |
| `chunks[].end` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `9007199254740991` |
| `source_length` | `integer` | 是 | — | 最小值 `0`；最大值 `1048576` |
| `mode` | `"characters"` / `"paragraphs"` / `"tokens"` / `"tokens-estimate"` | 是 | — | — |
| `estimated` | `boolean` | 是 | `false` | — |
| `tokenizer_id` | `string` | 否 | — | 最短 `1`；最长 `128` |
| `tokenizer_version` | `string` | 否 | — | 最短 `1`；最长 `128` |
| `estimator_id` | `"utf8-div4-v1"` | 否 | — | — |
| `estimator_version` | `"0.1.0"` | 否 | — | — |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

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

- overlap<size; offsets always Unicode scalar [start,end), original slices unchanged.
- Estimated tokens never guarantee actual Embedding tokenizer limit.
- Exact tokens mode independently re-encodes every final source slice with the pinned tokenizer; shrink on lossless source boundaries to <=size or fail, no implied guarantee from full-text token windows.

落地门槛：`unicode-split-library`、`optional-tokenizer`、`token-estimator-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:4c8f16d9a7717000af007cbac0708722b37245ee2df6ae630ff683b7f0c18a1a`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
