# url.parse接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/parse/input.schema.json) |
| 输出 | [output.schema.json](contracts/parse/output.schema.json) |
| 流式 | [stream.schema.json](contracts/parse/stream.schema.json) |
| 语义 | [semantics.json](contracts/parse/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `url` | 必填绝对HTTP/HTTPS URL，最多8192 Unicode码点 |
| 输出 | `scheme` | 小写`http`或`https`，不包含冒号 |
| 输出 | `host` | 规范化小写ASCII域名或标准IP文本；IPv6表示规则由固定解析器定义 |
| 输出 | `port` | 显式端口整数，未声明为null；不因默认端口而假装输入显式声明 |
| 输出 | `path` | 保留合法百分号编码的路径，空HTTP路径规范为`/` |
| 输出 | `query` | 原顺序的`{name,value}`数组，最多1000对；重复参数不合并 |
| 输出 | `fragment` | 不含`#`的片段；未出现`#`为null，显式空片段`#`为空字符串 |

## 精确语义

拒绝控制字符、非法百分号转义、无效UTF-8、缺失主机、非法端口和任何userinfo。协议只允许HTTP/HTTPS。输入不自动trim，调用者需要去空白时先用文本规范化能力。

域名大小写和IDNA转换、IPv4/IPv6规范形式及点路径处理须固定在不可变解析器版本中。不得靠字符串切分冒充完整URL解析。端口范围1..65535；输出port区分未声明与显式默认端口。

query按`&`分项、首个`=`分键值，无`=`时值为空串；按表单URL编码单次解码百分号并将`+`解释为空格。空query得到空数组，重复键保留顺序；非法编码整次失败。path和fragment不复用query的加号规则，不二次解码。

host必须非空且最多253码点；path及非null的fragment最多8192码点，query.name/value各最多4096码点。fragment保留合法百分号编码，不因`+`而改变文本。字段长度与1MiB完整UTF-8 JSON预算同时检查。

不返回含凭证的原URL副本。解析结果不判定主机是否可访问、是否公网或是否安全；DNS和网络策略必须在真正发送请求前由受控网络层重新执行。

结果无流式事件，超限或非法URL整次失败。多URL通过核心异步batch提交，不在本微插件内增加HTTP重试或fallback。
