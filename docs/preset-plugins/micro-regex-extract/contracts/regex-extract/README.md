# `text.regex-extract` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`text.regex-extract`；操作目录：`regex-extract`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `text` | `string` | 是 | — | 最短 `0`；最长 `262144` |
| `pattern` | `string` | 是 | — | 最短 `1`；最长 `4096`；UTF8<=4096bytes |
| `flags` | `string` | 否 | `""` | 最短 `0`；最长 `3`；仅ims且不重复 |
| `max_matches` | `integer` | 否 | `100` | 最小值 `1`；最大值 `1000` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `matches` | `array` | 是 | — | 最少项 `0`；最多项 `1000` |
| `matches[].value` | `string` | 父级存在时 | — | 最短 `0`；最长 `262144` |
| `matches[].groups` | `array` | 父级存在时 | — | 最少项 `0`；最多项 `128` |
| `matches[].groups[]` | `string` 或 `null` | 随数组 | — | — |
| `matches[].start` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `9007199254740991` |
| `matches[].end` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `9007199254740991` |
| `truncated` | `boolean` | 是 | `false` | — |

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

- Offsets Unicode scalars; zero-length must progress; no backrefs/lookbehind/recursive catastrophic engine.
- Validate UTF8 field byte lengths independently of JSON Schema codepoint maxLength: text<=262144 bytes and pattern<=4096 bytes; flags only unique i/m/s, capture groups<=128.

落地门槛：`linear-regex-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:b3f65344d9f24fa12baeb300d4d45d1fc7d136094c8d7eede638268564930193`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
