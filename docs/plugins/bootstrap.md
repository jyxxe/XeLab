# 插件启动环境与传输绑定

[插件目录](README.md) · [RPC](../rpc/README.md)

> 本版互操作 profile 采用受控网络内 mTLS gRPC；所有 SDK 必须支持。UDS 仅可作为经过独立 peer/generation 验收的本机实现扩展，不替代下面必需 profile。

## 固定监听与宿主入口

插件 RPC 在容器网络命名空间 `0.0.0.0:7000` 监听，必须实现 PluginLifecycle，按清单实现其他服务。HTTP/static/WebUI 如声明路由，在 `0.0.0.0:8080` 监听；不需要 HTTP 时禁止启动该入口。

7000和8080均只面向受控内核，后端连接采用实例mTLS验证内核来源。浏览器只连接公开网关，不直接连容器8080；HTTP Skill上下文由核心重新签发/建立。

内核 HostService 监听 server.host_rpc_listen，位于 runtime.network_cidr 对应的受控插件网络，仅批准实例可连接。该地址由内核注入，不通过插件自报注册。实际容器地址由运行时观测，网关不接收第三方提供的任意代理 URL。

## 注入环境变量

| 名称 | 格式与必需行为 |
|---|---|
| XELAB_PLUGIN_ID | 清单 plugin_id |
| XELAB_PLUGIN_VERSION | 精确 SemVer |
| XELAB_DEPLOYMENT_ID | UUID；只作一致性检查，不能代替通道身份 |
| XELAB_INSTANCE_ID | UUID；绑定当前证书及容器标签 |
| XELAB_GENERATION | 正整数；必须与 Initialize 一致 |
| XELAB_HOST_RPC_ENDPOINT | `https://<受控宿主地址>:<port>`；插件只能通过此入口调用宿主能力 |
| XELAB_TLS_CERT_PATH | `/run/xelab/identity/tls.crt`，当前实例证书 |
| XELAB_TLS_KEY_PATH | `/run/xelab/identity/tls.key`，私钥仅实例可读 |
| XELAB_TLS_CA_PATH | `/run/xelab/identity/ca.crt`，平台工作负载 CA |
| XELAB_PLUGIN_RPC_LISTEN | 固定 `0.0.0.0:7000`；清单不能覆盖 |
| XELAB_HTTP_LISTEN | 声明 HTTP 时固定 `0.0.0.0:8080` |

环境中不注入用户密码、API Key、Secret 原值、数据库 DSN 或运行时 socket。业务普通配置通过 Initialize.config_json 交付；敏感凭证按 SecretBinding/LeaseSecret 获取。

## 证书身份与轮换

实例证书 SAN 使用 `spiffe://xelab.local/instance/<instance_id>/generation/<generation>`；内核服务为 `spiffe://xelab.local/kernel`。证书 TTL 3600s，提前1800s原子轮换文件，SDK 重新加载用于新连接；旧连接仍须逐调用检查当前实例和授权状态。

实例停止或代次失效后即使证书未到期也不能获得授权。SAN 是经平台 CA 验证的身份来源，正文中的同名字段仅用于匹配，不能独立赋予身份。工作负载 CA 密钥由宿主保护，所有插件只挂只读 CA 证书。

启动后必须等待 Initialize 成功才接受业务调用。Describe/GetHealth 不能因依赖未 Ready 而永久阻塞；应及时返回当前不可用原因，使内核能完成诊断和恢复。
