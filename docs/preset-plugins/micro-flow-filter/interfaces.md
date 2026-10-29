# flow.filter接口草案

[插件规划](README.md) · [共享条件语义](../micro-flow-condition/interfaces.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。输入输出以本目录机器契约为准。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/filter/input.schema.json) |
| 输出 | [output.schema.json](contracts/filter/output.schema.json) |
| 流式 | [stream.schema.json](contracts/filter/stream.schema.json) |
| 语义 | [semantics.json](contracts/filter/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `items` | JSON数组，最多10000项；元素可为任意受限JSON值 |
| 输入 | `condition` | 对每个元素求值的共享条件AST |
| 输出 | `items` | 匹配元素组成的完整数组，保持原输入顺序与重复项 |
| 输出 | `indices` | 与输出元素一一对应的原数组下标，从0开始 |
| 输出 | `input_count` / `matched_count` | 输入项数与匹配项数 |

JSON Pointer采用空串表示当前元素，`/`分段，`~0`和`~1`分别转义`~`与`/`；数组下标使用规范十进制数字。只读取当前JSON值自身成员，不读取原型或宿主属性。

## 条件AST

组合节点为`{op,args}`：`and`、`or`的args长度1..32，`not`恰为1。叶节点使用`{op,path,value}`；`exists`与`is_null`省略value。允许`eq/ne/lt/lte/gt/gte/exists/is_null/in`，不支持动态函数或变量插值。

AST最多256节点、深度16。`in`的value为最多256项的字面量数组。相等采用JSON类型敏感的结构相等；大小比较仅对同类型有限数值或字符串成立，其他类型组合返回false，不自动转换数字字符串。字符串按Unicode码点顺序比较，对象成员顺序不影响结构相等，数组顺序有意义。

路径缺失时，除`exists`返回false外，所有叶条件均返回false；`is_null`只匹配显式null。`not`对布尔求值结果取反。and/or按args顺序短路，短路不能免除AST本身的完整合法性检查。

非法AST、预算不足或结果超限时整次失败，不返回部分数组。成功且无匹配项返回空数组。业务失败遵循声明的错误映射，不在stdout混入错误文本。

## 调用与批量

无增量输出；流式契约禁用。通过微部署生成的Skill调用，业务输入不能指定运行时、代码、版本、主体或权限。多次独立筛选走核心异步batch，每项仍是有界的一次纯JSON计算。
