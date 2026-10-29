# `json.validate` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`json.validate`；操作目录：`validate`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `document` | `JsonValue`（引用） | 是 | — | — |
| `schema` | `object / boolean` | 是 | — | 数据形式的MicroBusinessSchema受限集合；不是远程ref许可 |
| `max_errors` | `integer` | 否 | `64` | 最小值 `1`；最大值 `256` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `valid` | `boolean` | 是 | `false` | — |
| `errors` | `array` | 是 | — | 最少项 `0`；最多项 `256` |
| `errors[].instance_path` | `JsonPointer`（引用） | 父级存在时 | — | — |
| `errors[].schema_path` | `JsonPointer`（引用） | 父级存在时 | — | — |
| `errors[].keyword` | `string` | 父级存在时 | — | 最短 `1`；最长 `64` |
| `errors[].message` | `string` | 父级存在时 | — | 最短 `0`；最长 `512` |
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

- Schema<=64KiB/256nodes/depth16; no remote refs/arbitrary regex.
- Errors never default-log source values.

落地门槛：`restricted-schema-validator-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:82cd6bcd7abca076e5d615a962250219d7f133cb03af2cb1470d812ae30866ba`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
