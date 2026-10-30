# Extension Plugin的固定Skill

[Skill目录](../README.md) · [扩展插件目录](../../extensions/README.md) · [核心扩展架构](../../architecture/extension-plugins.md)

> 规格版本：2.8.0；状态：待实现的设计；无已发布或自动启用的扩展Skill。

## 候选与来源

ExtensionDeployment固定扩展版本、宿主、配置和limits，初始disabled。核心按宿主明确声明的extension_point生成candidate_skill；两个模式都可生成候选，但生成不等于注册或启用，扩展部署和Skill的enabled分别检查。

目标kind=extension，固定extension_version_id、extension_deployment_id、package_digest与host_chain_digest；来源installation_id为完整宿主链实际根OCI安装。调用者只发符合业务Schema的JSON，不指定宿主、handler地址、源码、主体或运行时句柄。

扩展制品的所有者/发布记录和内容摘要独立验证，不能仅以宿主已安装或已签名批准候选。注册时业务输入/输出必须与固定扩展点相符，不能改完候选Schema后继续借原扩展摘要。

| 模式 | 业务处理 | 边界 |
|---|---|---|
| data | 已声明宿主适配器读取contribution.json并返回业务结果 | 严格JSON≤256KiB；不是原生模块、JS源码或模板求值入口 |
| skill | 固定已注册handler_skill_id及definition_digest，经核心转发处理 | 当前调用者仍需原handler Skill及目标动作，处理器不是宿主服务账户 |

微宿主的data扩展必须保持原MicroManifest输入/输出及副作用边界，MPR1.3仅提供只读extensionData。skill模式由监督器通过HostService.InvokeSkill请求核心复核，微VM不获得HostService、凭证或任意Skill路由；原profile和network_enabled继续约束能力。

## 授权交集

leaf扩展业务调用先查固定Skill的skill.invoke和真实leaf ExtensionPlugin上的extension.invoke，再沿typed宿主链检查extension.host.use。

| 宿主类型 | host.use作用资源 | 额外核验 |
|---|---|---|
| ordinary | 实际deployment | 当前安装、部署及扩展点批准，不索取无关capability.invoke |
| micro | 真实micro_plugin | micro.invoke、父MPR deployment上的micro.runtime.use、当前微设置 |
| extension | 真实extension_plugin | 当前扩展来源及固定链；仅实际执行父业务时才追加其invoke/Skill |

mode=skill再检查固定handler的原skill.invoke和业务目标权限；调用其他capability/Skill时分别检查原动作。所有步骤都受同一主体的账户祖先、API Key/源凭证、客户端及执行委托上限约束，不能沿链获取宿主或扩展owner权限。

## 固定链与运行快照

host_chain_digest固定逻辑宿主版本/部署/契约关系，不含运行实例generation或微资源settings。动态runtime_snapshot单独绑定当前物理根及逻辑代次；根重启、微资源重配置或句柄淘汰使旧准备失效，重新Prepare同一精确链，不重写稳定Skill摘要。

更换扩展host/version/config/limits须新部署和候选；旧batch、工作流和结果始终保留原Skill/摘要/宿主链，不按同名latest替换。完整强依赖、micro→MPR、扩展宿主及handler目标统一DAG检查，最多16层，不能用处理器绕回自己或祖先形成递归。

## 调用、批量与资源

核心按当前授权、固定扩展点和有效deadline解析物理根，通过ExtensionHostService.Prepare/Invoke/Release执行。逻辑扩展宿主没有独立RPC地址，准备句柄与链摘要不是授权票据。宿主基础先Ready，再拓扑准备扩展；失效waiting_host，停用按逆拓扑撤路由、取消和Release。

多项业务走核心异步batch，逐项有界执行；整条扩展/handler链共享根调用计数、剩余deadline、取消和幂等上下文，不创建整批事务。handler的副作用及结果未知状态不能被data/宿主的none标记覆盖；已发送的外部请求必须按证据核实，不能盲重放。

data缓存、解析及执行计入实际根预算；微根保持该微部署聚合250mCPU/256MiB与MPR父4C/4GiB限制，不沿扩展层复制。独立handler服务按其原实例/微组额度及活动槽准入，扩展本身不申请另一份OCI资源或借用内核4C/16GiB预留。

扩展不取得宿主UI origin或令牌，也不动态注入桌面脚本。若通过界面调用，仅使用明确获准的固定Skill绑定，并由核心按当前扩展链再次授权。
