# M3 HTTP 契约与调用来源持久化

[实施索引](README.md) · [声明式 HTTP](../communication/plugin-http.md) · [整体恢复](m2-platform-recovery.md)

HTTP 集成使用追加迁移 `0015_plugin_http_contracts.sql`，内部 Schema 为 **15**。
原始 `metadata.sql` 与已发布迁移 1–14 的内容和校验和保持不变。
此页只说明持久化与恢复边界；不能代替 HTTP 网关、真实 OCI 或 M3 端到端验收。

## 不可变制品与调用

安装首先复用固定 OCI 摘要、发布者信任及原始 Manifest 签名验证，再读取镜像内
`/xelab/http/openapi.json`。原始字节不超过 4 MiB，摘要须与所有签名 HTTP 路由相同。
操作、权限、Schema 和 HTTP Skill 候选由现有 `plugin_http::Registry` 验证。

`plugin_http_contracts` 按安装 UUID 保存原始字节和 SHA-256；禁止修改和删除，
完全相同的安装重放只核对已有值，不重写来源。网关从该记录与原清单重建 Registry，
不从插件 Describe、用户 URL、请求头或可变部署版本取得接口权威。
静态路由没有 HTTP 业务契约，因此不创建伪造 OpenAPI 记录。

编译后的 Registry 最多缓存四项，键固定安装 UUID、重新验证的原始摘要及当前清单内容摘要。
每次查找仍读取数据库并核对所有签名 HTTP 路由摘要；权限定义、当前信任、审批和用户授权
不进入缓存。冷编译单工排队并在阻塞工作线程执行，编译期间不持有缓存表锁。

`calls.http_binding` 固定安装、部署、实例代次、契约摘要、operationId、方法、路径和
required_action。实际实例仍使用既有 `provider_instance_id`。绑定首次写入后不可修改。
历史部署可能已升级，不能用部署当前安装替换历史调用固定的安装与契约。
HTTP Skill 的虚拟 ContractRef 仍使用既有 Skill.name/version/definition_digest 规则。

静态 GET/HEAD 使用 `plugin_http_requests` 占用同一实例物理并发池，不伪造业务 Call
或用户日额度。未知工作保留到真实实例围栏完成；恢复已停止所有旧进程后清除此瞬时账本。

## 现场授权与恢复

签名清单不授予用户权限。业务 required_action 仅为 `plugin.http.invoke` 或插件自身
命名空间的动作；审批、启用和调用重新检查现有 permission_definitions 及 plugin
资源类型，并继续由现有 Authorizer 验证当前主体、原始凭证来源和全部祖先。

`plugin_http_approvals` 保存当前路由审批凭证、peer、固定审批摘要与来源路径。
它是可重新审批的当前权限记录，与不可变的制品来源分开；新无关授权不能复活旧审批。
恢复保留原始契约和历史调用，删除当前 HTTP 审批，并沿用现有凭证/发布者撤销、
部署停用及恢复 Owner 门禁。重启或恢复不会自动批准或启用路由。

快照导出及导入验证原始摘要、精确操作和历史 Call 绑定。原生快照仍要求相同
PostgreSQL 主版本、精确迁移校验和、表及列布局；旧 Schema 14 快照须使用匹配的
可信恢复程序，不能直接声称支持跨版本物理恢复。实际 OCI 备份与升级核对镜像成员
和已保存字节相同；未完成的正式 AWS/独立宿主验收保持独立待验收状态。

## 可执行验证入口

```sh
cargo test -p xelab-kernel --lib http_contracts
cargo test -p xelab-kernel --lib metadata_snapshot
cargo test -p xelab-kernel --test postgres
```

PostgreSQL 测试要求 `XELAB_TEST_DATABASE_URL`。覆盖真实 14→15 追加迁移及重复执行、
旧校验和保持、不可变字节、当前动作定义、损坏来源拒绝、历史实例绑定，以及最大
4 MiB 契约的真实 COPY 往返和审批重置。它们是元数据组件测试，不等于 OCI 或生产验收。
