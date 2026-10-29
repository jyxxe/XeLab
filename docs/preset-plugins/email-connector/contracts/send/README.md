# `email.send` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`email.send`；操作目录：`send`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `transport_alias` | `string` | 是 | — | 最短 `1`；最长 `128`；固定批准的连接/路由别名 |
| `to` | `array` | 是 | — | 最少项 `1`；最多项 `100` |
| `to[]` | `string` | 随数组 | — | 最短 `1`；最长 `320` |
| `cc` | `array` | 否 | — | 最少项 `0`；最多项 `100` |
| `cc[]` | `string` | 随数组 | — | 最短 `1`；最长 `320` |
| `bcc` | `array` | 否 | — | 最少项 `0`；最多项 `100` |
| `bcc[]` | `string` | 随数组 | — | 最短 `1`；最长 `320` |
| `subject` | `string` | 是 | — | 最短 `0`；最长 `998`；拒绝CR/LF |
| `text` | `string` | 否 | — | 最短 `0`；最长 `1048576` |
| `html` | `string` | 否 | — | 最短 `0`；最长 `1048576` |
| `template_alias` | `string` | 否 | — | 最短 `1`；最长 `128`；固定批准的连接/路由别名 |
| `template_data` | `object` | 否 | — | — |
| `attachments` | `array` | 否 | — | 最少项 `0`；最多项 `10` |
| `attachments[].blob_id` | `string` | 父级存在时 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `attachments[].filename` | `string` | 父级存在时 | — | 最短 `1`；最长 `255`；安全显示名，不是路径 |
| `attachments[].content_type` | `string` | 父级存在时 | — | 最短 `1`；最长 `128` |
| `dedupe_key` | `string` | 是 | — | 最短 `1`；最长 `128` |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `message_id` | `string` | 是 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `status` | `"accepted"` / `"sent"` / `"failed"` / `"unknown"` | 是 | — | — |
| `provider_message_id` | `string` 或 `null` | 是 | — | — |
| `accepted_recipients` | `array` | 是 | — | 最少项 `0`；最多项 `300` |
| `accepted_recipients[]` | `string` | 随数组 | — | 最短 `1`；最长 `320` |
| `rejected_recipients` | `array` | 是 | — | 最少项 `0`；最多项 `300` |
| `rejected_recipients[]` | `string` | 随数组 | — | 最短 `1`；最长 `320` |

## 业务流

[stream.schema.json](stream.schema.json)。

不提供流式业务输出；`null` 为禁用流的契约标记，调用不得发送业务数据帧。

## 执行约束

[完整执行语义](semantics.json)。

| 项目 | 定义 |
|---|---|
| 副作用分类 | `non_repeatable` |
| 最大输入 / 输出 | 4194304 / 16777216 字节 |
| JSON 深度 | ≤64 |
| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |
| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |
| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |

补充语义与跨字段约束：

- Total recipients<=100 and attachments total<=64MiB before provider lower limits/encoding expansion.
- Bcc private, no arbitrary sender/DSN/headers; partial acceptance blocks blind resend.

落地门槛：`email-transport-profile`、`delivery-evidence-recovery`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:1a581100c54e627664bd26f2e4bb18a182d3fbd83dc9fcda9f7a2cec80d148b0`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
