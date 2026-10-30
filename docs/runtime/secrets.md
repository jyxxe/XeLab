# 密钥托管与租约

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 写入与元数据

HTTP secret 接口仅写入明文或读取元数据，没有用户原始密钥读取接口。解码内容最多 64 KiB。平台使用宿主保护的主密钥进行认证加密，版本化密文保存在元数据库/密钥存储；主密钥不随数据库备份明文保存。

SecretSpec 只在 TLS 入口短暂处理，禁止写入审计、OpenTelemetry 属性或幂等响应缓存。写入返回 Secret 元数据，不回显 value_base64。

## 绑定与获取

SecretBinding 将 secret_id 与 deployment_id 关联，审批同时检查操作者 secret.bind 和插件所需范围。HostService.LeaseSecret 只接受绑定部署当前活跃实例，不接受正文自报 instance_id。

租约最长 900s，默认 300s，返回值不超过 64 KiB，实例按租约约定使用并禁止日志记录。LeaseSecret 依据服务内部权限，不自动向最终用户授予密钥原始读取权。

## 撤销与轮换

撤销绑定阻止新的租约；轮换递增 version，后续租约取得新版本。插件已获知的原始第三方凭证不能靠本地 lease 到期强制遗忘，必须结合外部凭证撤销/轮换。需要更强限制时使用不暴露原值的宿主外部 API 代理。

secret 元数据归档保留审计引用；后代 API Key 不因拥有 capability.invoke 就能列出或读取绑定 secret。密钥主密钥恢复属于受保护宿主运维流程。

## 微插件HTTP的无原值代理

MPR1.1.0的controlled-http-v1使用核心HttpBinding绑定微部署endpoint_alias与secret引用，Runtime和VM均不获得原值租约。pure-json-v1继续没有HTTP桥；MPR申请可选micro.http.request也不使pure脚本取得网络或secret能力。

MPR1.2.0增加微部署network_enabled手动开关，默认false；HTTP profile在开关关闭时于业务Invoke前拒绝，代理还逐次检查当前开关与部署generation。Prepare/Ready不触发联网。开关开启也不能跳过当前Secret/绑定/出口审批；已发送请求取消后按证据处理，未知结果不重放。

创建/替换HTTP绑定由micro.http.bind管理，创建或换secret另需secret.bind及父egress审批。核心依据已批准绑定限定的secret.use目的、当前secret状态/版本及完整微调用授权执行代理使用；终端不因调用此微插件而获得secret原值或LeaseSecret权限，MPR也不申请通用secret.use。

每个业务Call冻结enabled绑定版本、顺序与secret引用/版本，但冻结不免除当前检查。绑定禁用/删除、secret撤销、权限失效立即阻止后续请求；绑定或secret版本实质变化不能让旧Call换用新key，按STATE_CONFLICT/当前状态错误处理，需新Call获得新快照。priority变更只影响后续Call。

HostService.ProxyHttp从实际MPR工作负载和RpcDelegation推导调用，只对固定HTTPS443 endpoint、方法、路径和鉴权模板注入凭证；VM只提交经payload_schema校验的业务payload。原值不能进入VM、MPR、用户输入、代理台账、审计、诊断或幂等缓存。

桥响应不包含上游headers/Set-Cookie或非2xx原文；若2xx JSON任意字符串包含本次注入凭证原值，拒绝为UPSTREAM_RESPONSE_INVALID并只返回脱敏错误。日志只记secret引用/版本及代理operation_id，不记录注入头或原body。

代理能阻止后续凭证使用，不能撤回已经发送到上游的请求或强迫上游遗忘凭证；已发结果不明时保留unknown并禁止脚本自动重跑。需要外部彻底撤权仍结合提供者侧撤销/轮换。详见[MPR受控HTTP接口](../preset-plugins/micro-plugin-runtime/interfaces.md)。
