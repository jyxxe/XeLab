# 受控 HTTP 客户端接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `http.request`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `policy_alias` | string，必填 | 已批准的网络与认证配置；不能由输入扩大范围 |
| `url` | 绝对URL，必填 | 目标地址，须通过策略允许的协议、主机、端口与路径校验 |
| `method` | `GET/HEAD/POST/PUT/PATCH/DELETE`，默认GET | 受策略允许的方法集合进一步收窄 |
| `headers` | 字符串映射，可省略 | 有界的业务头；拒绝CR/LF、Host、连接控制与认证保留头 |
| `body_text` / `body_blob_id` | string / UUID，均可省略 | 至多一个；Blob≤64MiB，GET/HEAD禁止请求体 |
| `content_type` | string，可省略 | 声明体类型，不据此跳过内容检查 |
| `response_mode` | `json/text/blob`，默认json | JSON/文本内联或完整导出核心Blob |
| `timeout_ms` | 100..120000，默认30000 | 受根调用剩余deadline收窄 |
| `max_response_bytes` | 1..67108864，默认1048576 | 读取硬上限；内联模式还受16MiB JSON上限 |

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `status` | integer | 最终HTTP状态，4xx/5xx也是有效远端响应 |
| `headers` | 字符串映射 | 过滤认证、Cookie及逐跳敏感字段后的响应头 |
| `final_url` | string | 完成所有允许重定向后的地址 |
| `body_json` / `body_text` / `body_blob_id` | JSON值 / string / UUID | 按模式恰好一个；HEAD仅可选text并返回空正文 |
| `size_bytes`、`digest` | integer / string | 完整响应体大小与sha256摘要 |

json模式只接受合法有界JSON响应；不能把解析失败伪装为空对象。大响应只能使用blob模式，并受64MiB硬上限约束。

## 网络与重试

每次连接及每次重定向都重新应用SSRF、DNS解析结果、目标IP、端口、TLS证书和认证转发策略。默认拒绝环回、私网、链路本地、云元数据及非批准协议；代理和重定向不能扩大白名单。跨origin不转发凭证；redirect最多3次。

调用方不能传裸Authorization、Cookie、代理凭证或关闭证书校验；业务body也不能充当额外的认证凭证通道。受控Secret注入依赖核心绑定；日志和结果不能回显秘密。HTTPS之外的协议需要单独明确批准，不能由URL隐式开启。

达到字节上限即中止并报 `RESOURCE_EXHAUSTED`，不返回伪完整正文。网络错误、HTTP状态和业务JSON错误分开记录；对可能产生副作用的方法，响应丢失即结果不明，不能自动重发。

## 原始契约

- `http.request`： [输入](contracts/request/input.schema.json) · [输出](contracts/request/output.schema.json) · [流](contracts/request/stream.schema.json) · [语义](contracts/request/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

