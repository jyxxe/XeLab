# `text.encoding-convert` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`text.encoding-convert`；操作目录：`encoding-convert`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `data` | `string` | 是 | — | 最短 `0`；最长 `1048576`；严格base64字节，不是宿主路径 |
| `from` | `"utf-8"` / `"utf-16le"` / `"utf-16be"` | 是 | — | — |
| `to` | `"utf-8"` / `"utf-16le"` / `"utf-16be"` | 是 | — | — |
| `errors` | `"strict"` / `"replace"` | 否 | `"strict"` | — |
| `bom` | `"omit"` / `"emit"` | 否 | `"omit"` | — |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `data` | `string` | 是 | — | 最短 `0`；最长 `1048576`；base64输出 |
| `encoding` | `"utf-8"` / `"utf-16le"` / `"utf-16be"` | 是 | — | — |
| `replacement_count` | `integer` | 是 | — | 最小值 `0`；最大值 `1048576` |

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

- BOM must match source declaration, no charset guessing/host locale fallback.

落地门槛：`charset-codec-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:3e9686463a7589267170cfcc3cb5a4ea558a3a55d3ec76b1d7e4b65c31941f7f`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
