# 浏览器自动化接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `browser.navigate`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `browser_alias` | string，必填 | 已批准的浏览器、出口和凭证配置 |
| `url` | 绝对URL，必填 | 策略允许的导航目标 |
| `wait_until` | `domcontentloaded/load`，默认domcontentloaded | 有界等待条件，不承诺无限network-idle |
| `timeout_ms` | 100..60000，默认30000 | 根deadline内的导航总上限 |

输出 `session_id/final_url/title/expires_at`。session_id为不可猜测的短期标识，绑定可信主体、固定部署、批准配置和原授权链；建议空闲TTL 60秒，最长存活300秒，均为未实测配置建议。final_url记录最终批准地址，不能含凭证或认证令牌。

## `browser.extract`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `session_id` | string，必填 | 同主体、同部署且未过期的会话 |
| `fields` | 1..32项，必填 | 每项含 `name/selector`及可选`attribute/multiple` |
| `fields[].name` | 唯一string | 输出字段名 |
| `fields[].selector` | string | 受支持的CSS选择器，不接受JS表达式 |
| `fields[].attribute` | string，可省略，禁止null | 省略取文本；提供时取允许的DOM属性 |
| `fields[].multiple` | boolean，默认false | false取首个匹配或null；true返回匹配值数组 |
| `max_items` | 1..1000，默认100 | 每个多值字段的元素上限 |

输出 `session_id/final_url/data`，data按fields.name存储字符串、null或字符串数组。不存在节点不等于错误；选择器无效返回 `INVALID_ARGUMENT`；超过max_items拒绝该项，不静默丢尾。提取完成后会话仍仅在TTL内可用，不支持跨主体转交。

## 会话、网络与资源

页面导航、重定向、iframe、XHR、脚本和其他子资源请求均需执行相同出口、DNS/IP/TLS、凭证和资源额度校验；仅检查最初URL不足。禁用下载、任意协议处理、宿主文件、扩展和任意JS eval；禁用不代表允许未被拦截的后台网络。

凭证由核心按批准配置提供受控适配，不能出现在业务JSON、日志、DOM导出或结果中。会话有每主体及部署总数限制，到期/取消后关闭页面并清理Cookie和存储。导航失败不保留一个可误用的已授权session；已失效会话返回 `RESOURCE_NOT_FOUND`。

## 原始契约

- `browser.navigate`： [输入](contracts/navigate/input.schema.json) · [输出](contracts/navigate/output.schema.json) · [流](contracts/navigate/stream.schema.json) · [语义](contracts/navigate/semantics.json)
- `browser.extract`： [输入](contracts/extract/input.schema.json) · [输出](contracts/extract/output.schema.json) · [流](contracts/extract/stream.schema.json) · [语义](contracts/extract/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

