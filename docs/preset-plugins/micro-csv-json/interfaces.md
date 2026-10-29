# CSV / JSON 转换接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `convert.csv-json` / `csv-json`

[输入 Schema](contracts/csv-json/input.schema.json) · [输出 Schema](contracts/csv-json/output.schema.json) · [流式声明](contracts/csv-json/stream.schema.json) · [行为契约](contracts/csv-json/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `csv` | 必填 UTF-8 JSON 字符串 |
| `delimiter` | 可选 `,` / `;` / tab，默认 `,`；不支持其他分隔符 |
| `header` | 可选布尔值，默认 true；首条记录是否为列名 |
| `columns` | 可选唯一非空列名数组；`header=false` 时必填，`header=true` 时禁止 |
| `null_value` | 可选字符串；提供时精确匹配的字段映射为 JSON null，否则保留字符串 |
| `max_rows` | 可选整数，默认 1000、范围 1..10000；不含标题行 |
| `max_columns` | 可选整数，默认 128、范围 1..128 |

输出 `{columns: string[], rows: object[], row_count: integer}`。每行键与列名完全一致，值为字符串或 null；不做类型推断。重复或空列名、列数不一致、无闭合引号、超限数据均整体失败。空字符串默认保持空字符串，只有显式 `null_value=""` 才转换为空值。

## `convert.json-csv` / `json-csv`

[输入 Schema](contracts/json-csv/input.schema.json) · [输出 Schema](contracts/json-csv/output.schema.json) · [流式声明](contracts/json-csv/stream.schema.json) · [行为契约](contracts/json-csv/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `rows` | 必填对象数组，最多 10000 行；值仅字符串、数字、布尔值或 null |
| `columns` | 必填唯一非空列名数组，最多 128 列；固定输出顺序 |
| `delimiter` / `header` | 同上，默认 `,` / true |
| `null_value` | 可选字符串，默认空字符串；null 的输出表示 |

输出 `{csv: string, row_count: integer}`。缺失列作为 null；列清单外的输入键拒绝，不能静默丢弃。字符串原样编码，有限数字和布尔值用 JSON 标量文本；对象与数组值拒绝。

两接口按双引号包围含分隔符、引号或换行的字段，并将内部 `"` 写作 `""`。输出记录终止符为 CRLF；输入接受 CRLF 或 LF，字段内换行只允许出现在引号内。首个 UTF-8 BOM 可被剥离，其他字符不隐式修剪。

转换不执行公式，也不承诺生成内容可安全地在任意电子表格中启用主动功能。v1 返回一次完整 JSON；全部行列、解析中间结构和序列化结果受 1MiB 输入/输出及运行预算限制，超限整体失败。
