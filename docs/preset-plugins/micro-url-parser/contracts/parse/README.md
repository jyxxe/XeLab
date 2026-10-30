# `url.parse` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`url.parse`；操作目录：`parse`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `url` | `string` | 是 | — | 最短 `1`；最长 `8192` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `scheme` | `"http"` / `"https"` | 是 | — | — |
| `host` | `string` | 是 | — | 最短 `1`；最长 `253` |
| `port` | `integer` 或 `null` | 是 | — | — |
| `path` | `string` | 是 | — | 最短 `0`；最长 `8192` |
| `query` | `array` | 是 | — | 最少项 `0`；最多项 `1000` |
| `query[].name` | `string` | 父级存在时 | — | 最短 `0`；最长 `4096` |
| `query[].value` | `string` | 父级存在时 | — | 最短 `0`；最长 `4096` |
| `fragment` | `string` 或 `null` | 是 | — | — |

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

- No HTTP call, no userinfo return, parsing success is not egress permission.
- Preserve query parameter order and duplicates.

落地门槛：`url-parser-library`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:298a99d0c2c21f3335ea6e8af2bc392f8176f3c2c59b81e138c38d19711838a4`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
