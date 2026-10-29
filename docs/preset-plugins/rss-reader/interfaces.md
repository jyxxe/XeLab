# RSS / Atom 阅读器接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `feed.read`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `url` | 绝对URL，必填 | 批准策略允许的RSS/Atom地址 |
| `http_policy_alias` | string，必填 | 固定HTTP出口与可选认证配置 |
| `max_items` | 1..1000，默认100 | 返回文章项上限，按源顺序取前项 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `source_url` | string | 经过允许重定向后的源地址 |
| `title` | string或null | feed标题 |
| `format` | `rss/atom` | 检测的格式 |
| `items` | array | 按源顺序的文章项 |
| `items[].id` | string | 源GUID/Atom ID；不存在时为稳定内容派生标识 |
| `items[].title`、`link` | string或null | 规范化标题与允许协议的文章链接 |
| `items[].published_at`、`updated_at` | UTC时间字符串或null | 无法解析或未提供时为null |
| `items[].summary` | string或null | 清理后的纯文本摘要 |
| `truncated` | boolean | 源中有更多文章未返回 |
| `warnings` | string数组 | 有界且不含凭证的规范化/清理提示 |

相对link按最终source_url解析，不访问这些链接；非HTTP(S)或不合法URL应为null并附有界warning，不将其变成后续自动访问许可。时间不明不能用当前时间替代；文章ID的派生规则须在实现契约中固定后保持稳定。

## 获取与解析

所有HTTP连接、重定向和认证使用批准的http.request子调用或等价受控适配，并传播同一主体、根deadline和预算。不能在插件里另开不受监管的抓取客户端。

限制源响应字节、XML深度、节点数、文本总量及时间。禁用DTD、外部实体、XInclude和自动网络读取，防止实体展开与SSRF。摘要去除HTML脚本、事件和危险链接；接口输出纯文本，不向WebUI交付未经清理的HTML。

HTTP失败、格式无效及资源超限使用核心错误；部分源格式问题可通过有界warnings报告，不把失败下载伪装成空feed。批量订阅源读取采用异步batch，不增加无限源列表的同步入口。

## 原始契约

- `feed.read`： [输入](contracts/read/input.schema.json) · [输出](contracts/read/output.schema.json) · [流](contracts/read/stream.schema.json) · [语义](contracts/read/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

