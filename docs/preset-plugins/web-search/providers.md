# 网页搜索固定适配器

[网页搜索目录](README.md) · [绑定配置](configuration.md) · [输出契约](../../skills/web-search/README.md)

> 规格版本：2.6.0；状态：待实现的适配器设计。官方接口核对日期：2026-10-29。

`http_endpoints`随不可变微版本发布。绑定只能从声明中选择端点；不能指定通配host、自定义base_url、路径后缀或认证模板。业务输入只接受`query`和`limit`，不得透传未知provider参数。

## Brave

| 项目 | 固定定义 |
|---|---|
| endpoint alias / provider | `brave` / `brave` |
| method | `GET` |
| HTTPS origin | `https://api.search.brave.com` |
| path | `/res/v1/web/search` |
| 凭证位置 | 核心代理注入`X-Subscription-Token`请求头 |
| 编码 | 将payload白名单字段按UTF-8编码为查询参数 |
| 响应 | `application/json`，只提取`web.results` |

payload_schema只接受对象`{q,count}`，`additionalProperties=false`。`q`来自原始query，要求1..600个Unicode code point且最多75个按空白分隔的词；不能是全空白。`count`为1..20的整数，来自归一化后的limit。认证信息不进入查询参数。

把`web.results[]`的`title`、`url`、`description`映射到统一items；缺失或null的`web.results`按空列表处理，已出现但类型错误的列表不能当成空列表。条目必须满足统一输出类型；缺失摘要时用空字符串。

结果顺序保持上游web结果顺序，最多返回limit项。Brave的其他结果分区、摘要生成、rich callback、图片、视频和建议不进入输出，不继续请求任何回调URL。

接口、查询上限、鉴权和字段定义依据[Brave官方Web Search接口](https://api-dashboard.search.brave.com/api-reference/web/search/get)。

## Tavily

| 项目 | 固定定义 |
|---|---|
| endpoint alias / provider | `tavily` / `tavily` |
| method | `POST` |
| HTTPS origin | `https://api.tavily.com` |
| path | `/search` |
| 凭证位置 | 核心代理注入`Authorization: Bearer <secret>`请求头 |
| 编码 | `Content-Type: application/json`，UTF-8 JSON请求体 |
| 响应 | `application/json`，提取`results`及可用usage元数据 |

payload_schema是禁止附加字段的对象，包含以下字段；常量字段必须通过const校验：

| 字段 | 约束 |
|---|---|
| `query` | 与公共query相同的非空白文本和长度限制 |
| `max_results` | 与公共limit相同，整数1..20 |
| `topic` | 固定`general` |
| `search_depth` | 固定`basic` |
| `auto_parameters` | 固定`false` |
| `include_answer` | 固定`false` |
| `include_raw_content` | 固定`false` |
| `include_images` | 固定`false` |
| `include_usage` | 固定`true` |

把`results[]`的`title`、`url`、`content`映射为统一items的`title`、`url`、`description`，保留顺序且最多limit项。缺失摘要使用空字符串；缺失或类型错误的results属于无效响应，不以空成功掩盖。合法空数组是成功。

固定basic避免服务商自动升级搜索深度；官方当前列出basic每次1 API credit、advanced每次2 credits。本预设不启用生成答案、原文和图片，也不调用Extract/Crawl接口。上游usage属于辅助元数据，不改变平台以请求数计量的usage定义。[Tavily官方Search接口](https://docs.tavily.com/documentation/api-reference/endpoint/search)

## 通用响应约束

1. 2xx成功正文必须是固定端点的JSON响应，解压后最多512KiB；未知Content-Type、非JSON或超过预算属于无效响应。非2xx按状态和规范化错误处理，不解析或转交原始错误正文；2xx正文若包含Secret原值，核心直接拒绝，不能转成可fallback的普通上游失败。
2. `items`只含标题、URL、摘要；上游原始错误正文、响应headers、认证细节和Secret不公开给调用者。服务商request ID仅可作为受控诊断元数据。
3. URL仅作为结果数据返回，必须是合法HTTP/HTTPS绝对URL；不允许凭返回URL追加网络请求。含用户名或密码的URL不进入结果。
4. 不同服务商分数不进行跨服务商混排；第一个成功服务商决定整份结果，不能拼接多个服务商的不同费用响应。
5. 服务商的地区、语言、日期和安全过滤语义各异；v1不把这些参数纳入公共Skill，也不声称具备额外过滤保证。

## 网络与凭证边界

代理逐连接校验HTTPS证书、DNS解析结果和实际目标IP。拒绝回环、链路本地、宿主、元数据和未授权内部网络地址，不允许重定向；即使上游返回新的Location或callback URL也不跟随。

脚本只能提交匹配相应payload_schema的值，不能添加请求头或覆盖代理编码。代理根据冻结绑定找Secret，发送前重新检查当前撤销状态；原始凭证仅在受信代理内短暂使用，不交给MPR监督器或VM。

认证头不会转发到任何结果URL；插件诊断和幂等缓存不得记录注入后的请求。上游返回的HTML或摘要在WebUI中按文本显示，不能作为可信脚本执行。
