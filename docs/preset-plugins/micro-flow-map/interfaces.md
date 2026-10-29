# 流程字段映射接口草案

[插件说明](README.md) · [JSON 投影](../micro-json-project/interfaces.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `flow.map` / `map`

[输入 Schema](contracts/map/input.schema.json) · [输出 Schema](contracts/map/output.schema.json) · [流式声明](contracts/map/stream.schema.json) · [行为契约](contracts/map/semantics.json)

输入 `{document: JSON, mappings: array}`，`mappings` 为 1..256 个规则，输出 `{document: object}`。

| 规则字段 | 类型与语义 |
|---|---|
| `to` | 必填非空 JSON Pointer；目标对象路径 |
| `from` | 源 JSON Pointer；与 `value` 恰好提供一个 |
| `value` | 字面 JSON 常量；即使是字符串也不解释为路径或代码 |
| `default` | 可选字面 JSON；只有 from 缺失时采用，显式 null 不触发默认值 |
| `type` | 可选 `identity` / `string` / `number` / `integer` / `boolean`，默认 `identity` |
| `required` | 可选布尔值，默认 true；没有源值或默认值时，true 失败，false 跳过规则 |

规则先完整校验，再读取源值、应用缺失默认值、执行指定转换，最后写入新对象。`value` 常量规则不得同时提供仅对源缺失有效的 `default`。`default` 的存在与它是否为 null 必须分别判断。

路径遵循 [JSON 投影](../micro-json-project/interfaces.md) 的纯 JSON 语义：源根路径可为空，目标只建立对象层级，数字 token 作为对象键；重复或祖先/后代重叠目标整体拒绝。目标 token 禁止 `__proto__`、`constructor`、`prototype`，源自有同名 JSON 键仍可读取。每条规则只读取原始输入，不读取其他规则的中间输出。

`identity` 保留 JSON 值；其他转换仅接受标量。`string` 对字符串原样保留、对有限数字/布尔值使用标准 JSON 标量表示；null 不自动转换为空串。`number` / `integer` 接受原有数字或严格 JSON 数字字符串，integer 还要求整数且精确表示；不修剪空白、不接受 NaN/Infinity/十六进制。`boolean` 只接受布尔值或精确字符串 `true` / `false`。不支持的转换整体失败，不隐式取真值或截断小数。

不执行表达式、条件语言或任意代码。结果、临时结构与转换均受单次资源限制；输出超出 1MiB、深度或时间/内存预算时整体失败，不输出部分映射。v1 一次返回完整 JSON，无流式响应。
