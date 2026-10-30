# `document.parse` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`document.parse`；操作目录：`parse`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `blob_id` | `string` | 是 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `format` | `"auto"` / `"pdf"` / `"docx"` / `"txt"` / `"html"` | 否 | `"auto"` | — |
| `options` | `object` | 否 | — | 拒绝未知成员 |
| `options.include_pages` | `boolean` | 否 | `true` | — |
| `options.max_pages` | `integer` | 否 | `200` | 最小值 `1`；最大值 `1000` |

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `format` | `"pdf"` / `"docx"` / `"txt"` / `"html"` | 是 | — | — |
| `text` | `string` | 是 | — | 最短 `0`；最长 `8388608` |
| `pages` | `array` | 是 | — | 最少项 `0`；最多项 `1000` |
| `pages[].number` | `integer` | 父级存在时 | — | 最小值 `1`；最大值 `1000` |
| `pages[].text` | `string` | 父级存在时 | — | 最短 `0`；最长 `8388608` |
| `paragraphs` | `array` | 是 | — | 最少项 `0`；最多项 `100000` |
| `paragraphs[].index` | `integer` | 父级存在时 | — | 最小值 `0`；最大值 `99999` |
| `paragraphs[].text` | `string` | 父级存在时 | — | 最短 `0`；最长 `1048576` |
| `paragraphs[].page_number` | `integer` 或 `null` | 父级存在时 | — | — |
| `metadata` | `object` | 是 | — | 拒绝未知成员 |
| `metadata.title` | `string` 或 `null` | 父级存在时 | — | — |
| `metadata.author` | `string` 或 `null` | 父级存在时 | — | — |
| `metadata.language` | `string` 或 `null` | 父级存在时 | — | — |
| `metadata.page_count` | `integer` 或 `null` | 父级存在时 | — | — |
| `warnings` | `array` | 是 | — | 最少项 `0`；最多项 `1000` |
| `warnings[]` | `string` | 随数组 | — | 最短 `0`；最长 `512` |

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

- No default OCR, Office macro or external HTML fetch.
- Output16MiB hard cap includes duplicated page/text; refuse overflow.

落地门槛：`document-parser-libraries`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:a29893085e6810eb2595d5dd370732f26328a1222ad0285850d549406d4408fd`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
