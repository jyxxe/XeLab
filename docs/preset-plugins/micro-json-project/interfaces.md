# JSON 字段投影接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。

## `json.project` / `project`

[输入 Schema](contracts/project/input.schema.json) · [输出 Schema](contracts/project/output.schema.json) · [流式声明](contracts/project/stream.schema.json) · [行为契约](contracts/project/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `document` | 必填，任意 JSON 值；只读源数据 |
| `fields` | 必填，1..256 个 `{from,to}`；两个字段均为 JSON Pointer 字符串 |
| `missing` | 可选，`error` / `omit` / `null`，默认 `error`；源路径缺失时的行为 |

输出为 `{document: object}`，包含按映射建立的新对象。

源 `from` 允许空字符串以选择根值；路径按 `~1`、`~0` 规则解码，只读 JSON 自有成员。数组只接受非负、规范十进制索引，`-` 不可读取。显式 `null` 是存在的值，不能当作缺失。

目标 `to` 必须是非空 Pointer，以对象层级构建；数字 token 也是对象键，不隐式创建稀疏数组。目标路径重复或存在祖先/后代重叠时整体拒绝，禁止依赖输入顺序覆盖。解码后的任一目标 token 为 `__proto__`、`constructor` 或 `prototype` 时拒绝；源数据中同名的纯 JSON 自有键仍可读取，绝不访问或修改原型链。

`error` 在任一源缺失时整体失败；`omit` 跳过该映射；`null` 写入显式空值。所有目标校验在产出成功结果前完成，不返回部分投影。

路径解析与复制均计入总期限及内存预算。v1 为一次完整 JSON 响应；输出超出 1MiB、深度或执行预算时整体失败。批量使用核心异步 batch，不在此接口增设无界同步循环。
