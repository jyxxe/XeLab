# 对象存储用户与权限边界

[对象存储目录](README.md) · [核心权限](../../identity/permissions.md) · [委托上下文](../../identity/delegation.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

## 命名空间归属

插件只信任经过认证的内核mTLS通道提供的RpcContext.subject_id。所有对象按`(subject_id,key)`定位，客户端不能在input、HTTP头、游标或文件metadata中指定另一个owner。

API Key沿用所属账户的subject，其动作/资源上限与当前来源链仍逐次检查。子账户拥有自己的命名空间；父账户、Owner/Operator/Auditor或同权限组成员不因关系或角色名称自动读取另一个subject的文件。本版没有跨用户共享ACL、公开对象或代表他人读写接口。

Guest只有在核心明确授予所需Skill、目标能力及Blob动作时才能调用；匿名不因存在Guest角色得到文件服务。删除/禁用账户后，原Key或旧委托不得继续发起新的读取、导出或写入；业务内容的保留/清理由明确管理策略处理，不借旧身份后台继续工作。

## 核心与插件联合检查

每次请求都需满足核心skill.invoke及原目标capability.invoke、固定安装/定义和当前插件批准；插件进一步检查可信subject下的业务对象与CAS。业务入口设为skill_required，不能用裸能力Invoke或任意插件HTTP路径绕过Skill层。

读权限组可以明确列举object-list、object-stat、usage-get；下载还需object-export及相应核心Blob动作。写权限组按需另加object-put、object-delete，不使用全局通配符自动包含所有未来接口。

put的blob_id在输入Schema使用`x-xelab-resource-kind: blob`，核心检查blob.read并形成本次精确白名单；插件只能经HostService.ReadBlob读取。不能仅因调用者知道UUID就读取，不能直接访问Blob宿主目录。

export通过HostService.WriteBlob创建当前执行主体所有的临时Blob；核心逐次检查当前来源、插件批准、Blob写入与资源预算。返回blob_id只用于获准核心下载流程，不是跨用户访问令牌。

## 配额、游标和重放

用户、部署与卷三层额度见[配置](configuration.md)。读取usage只返回自己的logical_bytes、reserved_bytes、object_count、quota_bytes和max_objects，不枚举其他用户或泄露全卷业务量。

增加子账户不会复制部署预算或卷硬容量。插件没有可信祖先存储预留协议，因此本版每用户逻辑配额不宣称是账户树累计配额；账户树调用额度仍由核心照常执行。

游标是有完整性保护的不透明分页状态，绑定subject和查询条件，不是长期数据授权。重放旧游标、成功业务结果或过期前创建的Blob引用时仍需核实当前来源和各自资源权限，不能用曾经授权代替当前授权。

配置、卷管理、备份/恢复和诊断访问使用各自核心管理权限，获得文件读写Skill不自动获得调高quota、挂载路径、读取备份或修改部署的权限。

## 共享实例的真实边界

同一OCI实例及卷可保存多个用户内容，逻辑隔离由受信任插件实现。它不构成对插件恶意代码的逐用户物理隔离：拥有卷读取权的插件程序理论上能读取整个卷。

需要更强隔离时，按用户或工作空间部署独立实例并使用独立卷，继续消耗已有插件池与宿主容量；不能声称单共享容器内每次鉴权能阻止恶意插件读盘。日志、审计和指标不得记录文件内容、原始委托、密钥或未经批准的完整key列表。
