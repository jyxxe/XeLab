# 网页搜索业务Skill

[Skill目录](../README.md) · [网页搜索预设](../../preset-plugins/web-search/README.md) · [执行与恢复](../../preset-plugins/web-search/execution.md)

> 规格版本：2.8.0；状态：待实现的动态微Skill契约，不是已经注册的服务。

本Skill由`org.xelab.web-search@1.0.0`微制品提供，依赖MPR 1.2.0及controlled-http-v1。核心创建MicroDeployment后才生成其candidate_skill；发布微版本本身不会生成Skill，本目录不提供静态skill.json来假定一个已存在的微部署。

## 绑定与入口

| 项目 | 定义 |
|---|---|
| Skill名称 | `<实际runtime plugin_id>.micro.<micro_deployment_uuid>.run`，由核心生成 |
| version | 固定微版本`1.0.0` |
| target.kind | `micro` |
| target绑定 | 固定微版本、micro_deployment_id和package_digest |
| mapping | `identity` |
| maximum_timeout_ms | 15000，包含排队和全部上游请求 |
| input_blobs / stream_item_schema | `false` / `null` |
| side_effect来源 | MicroManifest固定`non_repeatable`，不得当作none恢复 |

使用现有核心Skill调用入口，指定已注册、已授权的不可变Skill UUID。调用者不能只传`org.xelab.web-search`就让核心自动选择另一个脚本或部署。

创建后还需完成注册、审批、启用和HttpBinding配置。微部署出厂network_enabled=false，必须先在停用、disabled且排空状态经[设置接口](../../preset-plugins/web-search/configuration.md#微部署资源与联网设置)手动打开，再启用搜索；绑定启用不会替代联网开关。核心检查skill.invoke、micro.invoke、父部署micro.runtime.use、HTTP权限及所选绑定的当前权限和完整来源链；Skill可读不等于可执行。

开关关闭时Prepare/Ready仍允许，搜索Invoke在创建worker/发送前返回PERMISSION_DENIED（violations.field=network_enabled），不占sequence、不fallback，也不返回SEARCH_NO_PROVIDER。开关true仍只允许固定profile及绑定/平台策略批准的API代理，不提供任意fetch/socket。

默认resources为250mCPU/256MiB，所有该微部署worker共享逐微cgroup合计硬上限；默认活动并发1、硬上限4，JS heap默认16MiB为每VM子上限。资源由核心适配器实施，MPR无宿主cgroup权限，无每微OCI；父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），工作池3500mCPU/3758096384字节（3584MiB），控制CPU500m、控制内存128MiB及缓存256MiB，内存合计3968MiB。先受4个硬活动槽限制，默认最多4个不同微部署活跃，不借内核4C/16GiB。

手动修改resources或联网开关须先停用并排空，PUT settings携If-Match及Idempotency-Key；其余运行状态返回STATE_CONFLICT。更新generation/revision、旧句柄失效，原limits、微版本、父绑定与Skill定义不变，重新启用后按新设置准入。

## 输入

顶层是对象，禁止附加属性：

| 字段 | 类型 | 必填 | 约束 |
|---|---|---|---|
| `query` | string | 是 | 1..600个Unicode code point，最多75个空白分隔词，不能全空白 |
| `limit` | integer | 否 | 1..20，未提供时取10 |

code point数按原字符串计量，不按UTF-8字节、UTF-16 code unit或显示字形计量。词数按Unicode空白分隔的非空片段计量，连续空白不产生空词；不对query做隐式改写后放宽长度上限。

输入不接受provider、绑定别名、优先级、任意URL、headers、API Key、Secret ID、owner或运行时信息。服务商顺序取本微部署的核心HttpBinding配置和调用时冻结快照。

```json
{
  "query": "Rust async cancellation",
  "limit": 10
}
```

## 成功输出

顶层禁止附加属性，以下字段全部必填：

| 字段 | 类型 | 定义 |
|---|---|---|
| `query` | string | 本次原始query |
| `provider` | string | 最终成功服务商，枚举`brave`、`tavily` |
| `items` | SearchItem[] | 按该服务商返回顺序排列，最多limit项，允许空数组 |
| `count` | integer | 等于items长度，0..20 |
| `fallback_used` | boolean | 是否已经进入初始alias之后的候选attempt |
| `attempts` | SearchAttempt[] | 串行尝试记录，1..3项，保持实际顺序 |
| `usage` | SearchUsage | 本次外部请求计量 |

`SearchItem`是禁止附加属性的对象：

| 字段 | 类型 | 定义 |
|---|---|---|
| `title` | string | 服务商返回的标题，按文本处理 |
| `url` | string | 不含用户名/密码的合法HTTP/HTTPS绝对URL，只作为数据返回 |
| `description` | string | 摘要文本；上游无摘要时为空字符串 |

`SearchAttempt`是禁止附加属性的对象，字段全部必填：

| 字段 | 类型 | 定义 |
|---|---|---|
| `provider` | string | `brave`或`tavily` |
| `outcome` | string | `success`、`not_sent`、`rate_limited`、`upstream_error`、`auth_rejected`或`invalid_response` |
| `http_status` | integer或null | 收到有效HTTP状态时为100..599整数；未取得时为null |

`SearchUsage`是禁止附加属性的对象：

| 字段 | 类型 | 定义 |
|---|---|---|
| `external_requests` | integer | 0..3，已可能发送请求字节的attempt数，not_sent不计入 |
| `potentially_billed_requests` | integer | 0..external_requests，保守统计可能收费请求，不是货币金额 |

成功时最后一个attempt必须为success，其provider等于顶层provider；此前记录只能是允许fallback的失败。一个服务商的空items同样是成功，不继续找其他服务商填满limit。

```json
{
  "query": "Rust async cancellation",
  "provider": "tavily",
  "items": [],
  "count": 0,
  "fallback_used": true,
  "attempts": [
    {"provider": "brave", "outcome": "rate_limited", "http_status": 429},
    {"provider": "tavily", "outcome": "success", "http_status": 200}
  ],
  "usage": {"external_requests": 2, "potentially_billed_requests": 2}
}
```

## 错误与重放

| 错误/状态 | 含义 |
|---|---|
| `SEARCH_INPUT_INVALID` / HTTP400 | query空白、词数超限等业务输入不合法；不创建外部请求 |
| `SEARCH_NO_PROVIDER` / HTTP503 | 无已配置、启用且可使用的候选绑定 |
| `SEARCH_PROVIDERS_EXHAUSTED` / HTTP503 | 候选明确失败后没有可继续的服务商；不伪造成功空列表 |
| `SEARCH_RESPONSE_INVALID` / HTTP502 | 上游成功正文无效、超限或不符合统一输出约束 |
| `UPSTREAM_OUTCOME_UNKNOWN` / HTTP409 / Call unknown | 已发送请求但结果无法核实，可能产生费用；立即停止fallback |
| 核心输入、授权、来源链或出口策略错误 | 保留原平台错误，不能借fallback绕过 |
| 核心资源、期限或取消状态 | 按实际发送情况保留未知判定，不承诺外部请求被撤销 |

前四种错误通过MicroManifest.business_errors声明，`retryable=false`；脚本调用`host.fail(code,message)`，Runtime映射为MicroInvokeResponse.error。失败使用核心ErrorDetail，不返回形似成功的业务对象。原始上游错误正文、响应headers、Secret和认证头不公开；受权诊断可查询规范化attempt信息。

unknown由核心强制产生，脚本不得把它声明为普通业务错误。unknown锁定本Call的后续HTTP操作；脚本即使catch异常、报告其他业务错误或输出合法JSON，核心仍保留unknown。

每alias最多尝试一次、总最多三次且串行。发送后结果不明时v1没有ambiguous fallback开关。核心、Broker或HTTP库不能自动重发unknown调用；重新提交一个新业务查询可能再次收费，不能当作结果查询。

## 批量与WebUI

多条查询使用[核心异步Skill batch](../batches.md)，每项input遵守上述对象定义，目标是同一固定Skill UUID。批任务通过状态和结果查询获取完成项，unknown项不自动重放。

WebUI可用统一Skill表单展示query和limit，用任务中心展示批任务。结果标题、摘要按普通文本渲染，URL不会被核心或微插件自动抓取；本预设本身不发布iframe窗口、桌面组件或生成式答案接口。
