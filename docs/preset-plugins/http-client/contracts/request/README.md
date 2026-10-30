# `http.request` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`http.request`；操作目录：`request`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `policy_alias` | `string` | 是 | — | 最短 `1`；最长 `128`；固定批准的连接/路由别名 |
| `url` | `string` | 是 | — | 最短 `1`；最长 `4096`；格式 `"uri"`；每跳DNS/地址/策略校验 |
| `method` | `"GET"` / `"HEAD"` / `"POST"` / `"PUT"` / `"PATCH"` / `"DELETE"` | 否 | `"GET"` | — |
| `headers` | `object` | 否 | — | 最多成员 `32` |
| `body_text` | `string` | 否 | — | 最短 `0`；最长 `1048576` |
| `body_blob_id` | `string` | 否 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `content_type` | `string` | 否 | — | 最短 `1`；最长 `128` |
| `response_mode` | `"json"` / `"text"` / `"blob"` | 否 | `"json"` | — |
| `timeout_ms` | `integer` | 否 | `30000` | 最小值 `100`；最大值 `120000` |
| `max_response_bytes` | `integer` | 否 | `1048576` | 最小值 `1`；最大值 `67108864` |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

## 输出

[output.schema.json](output.schema.json)。

| 字段 | 类型 | 必填 | 缺省值 | 约束 |
|---|---|---|---|---|
| `status` | `integer` | 是 | — | 最小值 `100`；最大值 `599` |
| `headers` | `object` | 是 | — | 最多成员 `64` |
| `final_url` | `string` | 是 | — | 最短 `1`；最长 `4096`；格式 `"uri"` |
| `body_json` | `JsonValue`（引用） | 否 | — | — |
| `body_text` | `string` | 否 | — | 最短 `0`；最长 `16777216` |
| `body_blob_id` | `string` | 否 | — | 最短 `1`；最长 `36`；格式 `"uuid"`；UUID |
| `size_bytes` | `integer` | 是 | — | 最小值 `0`；最大值 `67108864` |
| `digest` | `string` | 是 | — | 最短 `1`；最长 `71`；模式 `"^sha256:[a-f0-9]{64}$"`；SHA256 |

条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。

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

- No raw credentials or unapproved destination via headers/body.
- Cross-origin redirects never carry original credentials; maximum3 and each target validated.
- Binary/large response uses Blob<=64MiB, not inline JSON.

落地门槛：`http-policy-binding`、`response-blob-streaming`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:cb5e34d3e3c75e9d1765857260cbd2f579e869aa0fa4a0eff76277b175940e8d`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
