# 流程条件接口草案

[插件说明](README.md) · [流程过滤](../micro-flow-filter/interfaces.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `flow.condition` / `condition`

[输入 Schema](contracts/condition/input.schema.json) · [输出 Schema](contracts/condition/output.schema.json) · [流式声明](contracts/condition/stream.schema.json) · [行为契约](contracts/condition/semantics.json)

输入 `{data: JSON, condition: object}`，输出 `{matched: boolean}`。条件只读取 `data`，没有副作用，也不实际执行分支。

## 共用条件 AST

| 节点 | 必填字段与语义 |
|---|---|
| `and` / `or` | `{op,args}`，args 为 1..32 个子节点；按顺序短路 |
| `not` | `{op,args}`，args 恰好含一个子节点 |
| `exists` | `{op,path}`，路径是否存在；显式 null 也存在 |
| `is_null` | `{op,path}`，路径存在且值为 null |
| `eq` / `ne` | `{op,path,value}`，JSON 值严格相等/不等，不隐式类型转换 |
| `lt` / `lte` / `gt` / `gte` | `{op,path,value}`，只比较两个数字或两个字符串 |
| `in` | `{op,path,value}`，value 必须为最多 256 项的数组，判断源值是否严格等于任一数组项 |

`path` 为 RFC 6901 JSON Pointer，空字符串代表根值；只读取自有 JSON 成员，数组索引必须是规范非负整数，不支持 `-`、通配符、表达式或原型链。`value` 是字面 JSON 数据，不解释为字段引用或代码。

所有比较节点在路径缺失时均为 false，包括 `ne`；用 `exists` 显式检测缺失。`eq` / `ne` 使用结构相等：对象键顺序无关，数组顺序有关，类型必须相同。数字只使用受支持的有限 JSON 数值；字符串顺序按 Unicode 标量字典序，不依赖区域设置。顺序比较类型不兼容时为 false。

`condition` 总节点最多 256、嵌套深度最多 16。即使分支会短路，必须先完整验证 AST；未知操作符、字段或不合法节点结构整体失败，不返回 `matched=false` 来掩盖语法错误。

不得调用知识库 `topk`、函数或任何动态执行接口。AST 构建与求值共用调用资源预算。v1 一次返回完整 JSON，无流式结果。
