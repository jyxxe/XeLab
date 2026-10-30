# flow.template接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/template/input.schema.json) |
| 输出 | [output.schema.json](contracts/template/output.schema.json) |
| 流式 | [stream.schema.json](contracts/template/stream.schema.json) |
| 语义 | [semantics.json](contracts/template/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `template` | 必填文本，最多262144 Unicode码点、4096个占位符；完整输入另受1MiB UTF-8预算约束 |
| 输入 | `data` | 占位符读取的JSON根值 |
| 输入 | `output` | `text`或`json`；省略按text解释 |
| 输入 | `missing` | 缺失路径策略`error`或`empty`；省略按error解释 |
| 输出 | `value` | text模式为字符串；json模式为解析成功的完整JSON值 |
| 输出 | `output` | 实际输出模式 |

Schema中的default仅作文档注解；上述省略行为由固定版本处理器显式实现。

## 模板语法

占位符规划为`{{ JSON_POINTER | FILTER }}`；JSON Pointer遵循`~0/~1`转义，空指针用空双引号标记。每个占位符最多一个过滤器，不允许表达式、循环、条件块或动态属性执行。`\{{`输出字面量`{{`，转义规则由固定解析器一次性处理。

过滤器集合固定为`text`、`json`、`html`。text只接受字符串、有限数字、布尔及null，null输出空串；json输出该值的合法紧凑JSON；html对text结果转义`& < > " '`。省略过滤器按text处理，不自动选择目标语言的转义策略。

缺失路径在error模式使整次失败，在empty模式输出空串；显式null按所选过滤器处理。对象与数组必须使用json过滤器，避免隐式字符串化。只访问数据自有成员，不执行getter或宿主对象方法。

json输出模式先完成渲染，再严格解析为单个JSON值；重复key、非法UTF-8、非有限数字、尾随内容或深度超限均失败。过滤器json产生的是值片段，不替代完整输出校验。

非法语法、未知过滤器、替换次数或结果超限时整次失败，不提交半截文本。错误只含有界位置和规则名，不默认回显敏感数据。输出无流式片段；多份模板使用核心异步batch。
