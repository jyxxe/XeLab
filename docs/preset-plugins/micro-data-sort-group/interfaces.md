# data.sort与data.group接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 入口 | 输入 | 输出 | 流式 | 语义 |
|---|---|---|---|---|
| data.sort | [input](contracts/sort/input.schema.json) | [output](contracts/sort/output.schema.json) | [stream](contracts/sort/stream.schema.json) | [semantics](contracts/sort/semantics.json) |
| data.group | [input](contracts/group/input.schema.json) | [output](contracts/group/output.schema.json) | [stream](contracts/group/stream.schema.json) | [semantics](contracts/group/semantics.json) |

## 共同数据范围

输入items最多10000条记录；键最多8个JSON Pointer，只访问当前元素自身JSON成员。键只支持字符串、有限数字、布尔和null；对象、数组键整次拒绝。数字遵循受限JSON安全数值规则，不将数字字符串转换为数值。

字符串按Unicode码点字典序比较；布尔false排在true之前；数字按数值比较。不同非null类型不能进行大小比较，必须在执行前检查全部记录，不因短路或排序实现碰巧未比较而放过类型冲突。

## data.sort

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `items` | 完整待排序数组 |
| 输入 | `keys` | 按优先级排列的`{path,order,nulls}`数组，1..8项 |
| 键 | `order` | `asc`或`desc`；省略按asc解释 |
| 键 | `nulls` | `first`或`last`；省略按last解释 |
| 输出 | `items` | 稳定排序后的完整数组 |
| 输出 | `indices` | 各结果元素对应的原输入下标，从0开始 |

路径缺失按null参与排序，与显式null处于同一空值档。nulls位置独立于order；desc只反转非null值比较。所有键相等时按原下标保序，重复元素保留，不修改原记录内容。

## data.group

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `items` | 完整待分组数组 |
| 输入 | `keys` | JSON Pointer字符串数组，1..8项 |
| 输入 | `max_groups` | 1..10000，省略按1000解释 |
| 输入 | `missing` | `error`、`skip`或`group-null`；省略按error解释 |
| 输出 | `groups` | `{key,items,indices}`数组；key为与keys顺序一致的标量值元组 |
| 输出 | `group_count` | 实际组数，与groups长度一致 |

相同类型且相同值的键归为一组，显式null是合法分组键。不同类型键保持不同组，数字1与字符串`"1"`不合并。missing=error拒绝任一缺失键，skip丢弃该记录并按固定语义记录处理，group-null把缺失键映射为null，允许与显式null合并。

组按键元组首次出现的顺序输出，组内按原输入顺序，indices保留原始下标。空输入得到空groups；超过max_groups整次失败，不溢出为其他组或返回部分结果。

default注解不自动填参，省略行为由固定处理器实现。预算或类型失败均不输出部分数据；两入口无流式结果。多次独立调用使用核心异步batch。
