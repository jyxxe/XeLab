# 扩展授权、来源与隔离

[扩展目录](README.md) · [身份权限](../identity/README.md) · [宿主](hosts.md)

> 规格版本：2.8.0；状态：待实现的安全契约。扩展是独立不可信来源，宿主可用不代表扩展获准。

## 独立资源与双方批准

扩展使用独立extension_plugin资源。逻辑身份、发布、读取贡献、部署、启停及执行分别授权；所有者来自核心认证上下文，输入中的owner_id不能改变归属。旧权限组、子账户或API Key的冻结上限不会因新增扩展动作自动扩大。

管理一个扩展不等于可以把它挂到任意宿主。创建部署还要核验实际typed host、宿主使用/扩展授权、固定版本和点契约；宿主所有者可限定可接受的扩展、处理器、模式和数据范围。反向地，拥有宿主管理权也不自动取得其他所有者的扩展源码或处理器使用权。

执行时联合检查当前调用者skill.invoke、扩展执行权、每一层宿主使用权、实际物理根及处理器所需权限，并与完整账户祖先、源凭证及当前委托上限求交。UUID、链摘要及已准备句柄都不能替代权限检查。

叶扩展检查extension.invoke；每一层typed宿主检查extension.host.use，ordinary按真实deployment授权，micro解析到真实micro_plugin，extension解析到真实extension_plugin。host.use只授权相应扩展点，不授予宿主管理、源码读取、Secret、网络或其他业务能力。

ordinary根还检查当前安装与出口审批，扩展点调用不强求一个无关的原capability.invoke；另调用既有能力或Skill时再检查对应动作。micro根额外检查micro.invoke及micro.runtime.use。经过祖先扩展的业务handler还要检查该扩展自己的invoke/Skill授权；所有固定handler都需要原skill.invoke及其实际目标动作。

## 来源链与不可变目标

普通OCI签名覆盖其原宿主清单和代码；微源码由其独立owner与包摘要登记；扩展贡献再由自己的发布记录与摘要固定。三者不能互相替代，不因运行在同一个MPR或属于同名产品就建立信任。

公共扩展Skill固定扩展版本、部署、package_digest和host_chain_digest；其来源记录明确为extension。即使CreateSkill.installation_id指向实际root安装，也不能据此把任意扩展当成宿主镜像内受签名候选。

核心读取真实对象构造宿主链、config、贡献和处理器引用，不接受调用方提交的root地址、mTLS身份、权限或任意代码。处理器摘要变更、绑定不匹配或祖先失效均拒绝，不能按最新版本自动修复。

## 禁止隐式能力扩张

data模式只解释宿主点的有界JSON语法；禁止JS/.so/原生模块加载、shell、任意路径、真实fetch/socket或运行时socket。配置Schema、贡献中的blob_id和URL不授予文件或网络权限。

skill模式只有宿主监督器的固定核心转发路径；业务JSON不能选择handler。委托缩限到该处理器、契约、数据范围和剩余期限，不能转发宿主或调用者的全部权限。处理器自己的Secret、数据及网络仍按其原规则使用，扩展不取得原值。

微宿主遵守原profile及resources/network_enabled。network_enabled默认false；受控HTTP根在关闭状态于业务执行前拒绝，不把开关关闭伪装成服务商故障fallback。pure微宿主不能借扩展或独立handler扩展为HTTP、文件或进程能力。

MPR1.3的host.extensionData只提供本次明确扩展点的只读JSON投影，不暴露RPC上下文、源凭证、实例证书、资源句柄或其他微VM状态。底层worker仍须核心Attach确认真实入组后才执行用户代码。

## 链、撤权与输出

强依赖图拒绝自环/间接环，链最多16层；运行extension_path还禁止同一扩展重入，不能用多层包装重置deadline、扩展权限或重复申请同一父资源。

逻辑closure和本次运行快照固定目标，但不冻结授权。停用、归档、祖先失效、Secret/绑定撤销及网络开关变化均按当前规则阻止后续执行或转发；已发请求只能按证据取消/核实，unknown不能被stdout成功覆盖。

宿主、扩展及处理器只得到其职责所需数据，日志按真实Call授权隔离。错误不泄露其他所有者贡献、宿主内部路径、凭证或原始堆栈；无权访问的扩展不能通过扩展列表或准备缓存命中被枚举。

扩展返回HTML、路径、表达式或下一步指令仍是数据。WebUI展示不得把它直接注入可信桌面DOM或当作宿主命令；可选UI贡献仍需独立声明、隔离来源和原有UI授权。

skill模式的宿主监督器还须其当前安装已批准的skill.invoke代理规则。MPR1.3为此增加可选申请，未批准时该模式不可用，data模式可独立使用。核心将每Call代理缩限到闭包中唯一handler ID/定义摘要，检查原调用者与处理器目标权限；MPR普通micro Call没有通用Skill桥，不能借父代理批准执行任意Skill。可选申请不会授予VM、扩展数据或终端调用者宿主权限。
