# file.metadata接口草案

[插件规划](README.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/metadata/input.schema.json) |
| 输出 | [output.schema.json](contracts/metadata/output.schema.json) |
| 流式 | [stream.schema.json](contracts/metadata/stream.schema.json) |
| 语义 | [semantics.json](contracts/metadata/semantics.json) |

## 声明模式字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `source` | 首版只支持`declared`；可省略并按declared解释 |
| 输入 | `descriptor.name` | 必填显示文件名，1..255 Unicode码点 |
| 输入 | `descriptor.size_bytes` | 必填声明大小，整数0..67108864；首版不接受未知大小 |
| 输入 | `descriptor.media_type` | 必填；声明MIME类型为1..128字符，未知为null |
| 输出 | `name` / `extension` | 原显示名称与小写后缀；无扩展名时extension为空字符串 |
| 输出 | `size_bytes` | 原声明大小，不冒充实测大小 |
| 输出 | `declared_media_type` | 规范化的声明MIME，未知为null |
| 输出 | `suggested_media_type` | 固定MIME数据库按后缀推断的候选类型；不可用为null |
| 输出 | `trust` | 首版恒为`declared` |

name只按显示名称解析；`/`、`\`、NUL和控制字符被拒绝，不解释相对路径、绝对路径或URL。隐藏文件`.env`无扩展名；`a.tar.gz`的末段扩展名为gz，不自动当作已确认归档格式。

descriptor及其三个字段均必填，不自动补大小或猜测声明MIME。size_bytes首版范围0..67108864，不执行实际文件读取；超过范围明确拒绝。media_type只接受语法合法的类型/子类型，按ASCII小写规范化，不用它授予上传或执行权限。

source省略行为须由固定处理器实现，default注解不自动修改输入。name和extension各最多255码点，MIME输出最多128字符，无法从固定数据库推断时suggested_media_type为null。

## 核实描述的未来门槛

`core-metadata`为待设计适配模式，不由首版Schema接收。未来核心先授权文件引用、核实对象版本并形成有来源的描述，再通过可信封装给微插件；正文中的`trust`、`verified`或伪造适配器ID均不能升级信任等级。

未来输出若标记verified，必须逐字段记录证据来源及检测器版本；内容MIME、声明MIME和后缀猜测应分别保留。单纯核心转发调用者描述仍是declared。

纯模式不接受host_path、文件句柄或任意Blob引用，不读文件签名。无增量输出；非法名称、类型或数值整次失败，批量走核心异步batch。
