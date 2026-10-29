# JSON 校验接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `json.validate` / `validate`

[输入 Schema](contracts/validate/input.schema.json) · [输出 Schema](contracts/validate/output.schema.json) · [流式声明](contracts/validate/stream.schema.json) · [行为契约](contracts/validate/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `document` | 必填，任意 JSON 值；待校验的数据 |
| `schema` | 必填，布尔值或受限 Schema 对象；必须先通过 MicroBusinessSchema 元校验 |
| `max_errors` | 可选整数，默认 64，范围 1..256；最多返回的错误条数 |

| 输出字段 | 类型与语义 |
|---|---|
| `valid` | 布尔值；文档是否满足所有校验条件 |
| `errors` | 错误数组；通过时为空；每项含 `instance_path`、`schema_path`、`keyword`、`message` |
| `truncated` | 布尔值；错误数量超过 `max_errors` 时为 true，且 `valid=false` |

两个 path 都使用 RFC 6901 JSON Pointer；根路径为空字符串。`keyword` 是失败的受支持关键词，`message` 只用于展示，调用方应依据路径和关键词处理错误。

先限制 Schema 字节、节点和深度，再建立校验结构，禁止通过构建过程绕过 64KiB / 256 节点 / 深度 16 限额。禁止 `$ref`、正则、任意代码及未声明扩展；未支持关键词须拒绝，不能静默忽略。遍历使用确定顺序，错误达到上限后可停止收集。

校验不修改 `document`，不自动填入 `default`，不做类型转换。输入或 Schema 越界、Schema 非法、执行超时均返回失败，不能伪装为 `valid=false` 的数据校验结果。v1 只返回一次完整 JSON；输出超限时整体失败，不截断成成功文档。
