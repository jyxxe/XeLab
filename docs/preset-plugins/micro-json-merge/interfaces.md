# JSON 合并接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `json.merge` / `merge`

[输入 Schema](contracts/merge/input.schema.json) · [输出 Schema](contracts/merge/output.schema.json) · [流式声明](contracts/merge/stream.schema.json) · [行为契约](contracts/merge/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `objects` | 必填，1..128 个 JSON 对象；从左到右合并 |
| `mode` | 可选，`shallow` / `deep`，默认 `shallow` |
| `array_policy` | 可选，`replace` / `concat`，默认 `replace`；控制冲突位置两个数组的处理 |

输出为 `{document: object}`，输入对象始终保持不变。

`shallow` 只合并顶层键，冲突值由较后对象替换；两个数组发生冲突时仍应用 `array_policy`。`deep` 对两个对象递归合并，数组按策略处理，其他类型冲突包括 `null` 均由后值替换。单个对象不会因顺序或键名意外获得原型属性。

`replace` 用后数组整体替换前数组；`concat` 依次保留前后所有元素，不去重、不按元素索引递归合并。对象键必须按纯 JSON 数据处理，禁止通过 `__proto__`、`constructor` 等键修改宿主原型或读取继承属性。

结果及处理中间结构共同受 heap 预算限制；执行前和每次扩展时检查深度及增长。达到时间、内存、1MiB 输出或深度 64 上限即整体失败，禁止返回部分合并或静默丢弃数据。

v1 只返回完整 JSON，流式不可用；若多个合并任务需排队，使用核心异步 batch，不把 `objects` 当作无界批处理入口。
