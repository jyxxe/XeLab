# Skill 双层授权与插件委托

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

普通目标允许执行必须同时满足skill.invoke、目标原有能力/HTTP动作、插件代理上限、当前主体与全部源约束，以及本次资源白名单。micro目标另按下述独立资源规则核对。注册/启用Skill不自动给任何用户授予执行权。

建议按用途发布权限组，将某个 Skill UUID 的 read/invoke 与其明确目标能力、必要 Blob 权限一起绑定。不能用一个 skill.invoke 全局通配符替代底层权限。

skill_required 在核心所有业务入口检查：HTTP /calls、HostService.Invoke、工作流裸 contract、模型别名和声明式 HTTP 必须具有合法 Skill 路由上下文。上下文只能由核心建立，插件或调用方不能靠请求头自行声明已通过 Skill。

用户调用知识库后，插件再调用 embedding Skill，HostService 继续取当前用户/执行身份、知识库获批代理权限及固定委托的交集；不能把知识库自身权限借给用户。

知识库无向量需求时，不批准 embedding Skill 代理权限也能使用表与标量查询。启用向量时额外批准指定 Skill/能力范围；媒体路径还需要精确 Blob 读取和受控临时 Blob 创建权限。

定时索引不能保存原始请求句柄无限运行。使用每批重新认证的用户请求，或明确批准的 indexer 服务账户；服务账户不能作为用户越权时的自动降级方案。

知识库缓存通过HostService.CheckSkillAccess校验当前调用资格，不能只用ResolveSkill的读取权限。字段可固定query/index不同主备集合，同空间fallback由知识库选择新的已批准固定Skill；SkillRouter本身仍不把一个Skill UUID自动换到另一实现。

## 微插件的对象授权

微执行须同时具有固定Skill UUID的skill.invoke、资源类型micro_plugin且匹配实际MicroPlugin UUID的micro.invoke，以及资源类型deployment且匹配父runtime_deployment_id的micro.runtime.use；还要通过MPR当前审批、固定安装及完整账户/祖先/源凭证/客户端/执行委托检查。不以MPR通用capability.invoke代替这两种对象权限。

MicroPlugin默认属于创建时经验证的owner；正文owner或脚本输出不能改变归属。Member模板可对本人微资源取得明确默认动作，仍需父MPR使用权及固定Skill调用权。子账户和API Key可按具体micro UUID取得部分或全部获准动作，仍与父级及来源链求交；父子关系、同一个MPR实例和可读取微版本都不自动授予执行或共享数据。

发布版本、创建微部署和注册候选Skill分别执行各自管理权限；创建微部署还要验证父部署micro.runtime.use，生成deployment.candidate_skill不发放执行权。registerSkill验证操作者可管理该来源owner/微对象、微版本发布来源及精确MPR安装，不能用MPR镜像签名给后来上传的代码背书。

内核把可信微目标送到MicroRuntimeService，MPR再次核对版本/部署/摘要和当前句柄代次。脚本只接收已验证业务JSON；两个profile均无文件、真实网络、secret原值、Blob或直接HostService能力，不能利用MPR自身代理权限继续调用其他插件。

Skill/微部署停用、微对象或微部署归档、授权链失效、父部署micro.runtime.use撤销或MPR审批撤销均撤销新执行并取消在途。准备句柄、缓存的编译代码和过去的成功调用都不是当前授权证明；每次执行及结果提交仍读取当前授权状态。详见[核心微插件对象](../architecture/micro-plugins.md)。

## 受控HTTP的附加授权

MicroManifest.execution_profile缺省为pure-json-v1，side_effect=none且http_endpoints为空；纯脚本不因MPR具备可选micro.http.request代理审批而取得HTTP桥。controlled-http-v1必须显式non_repeatable及不可变endpoint定义，新宿主能力需新包摘要和显式微部署审批。

HttpBinding读取需micro.http.read，写入需micro.http.bind；创建/换secret需具体secret.bind，并验证父Runtime egress审批。读取绑定元数据不授予原值读取。MicroDeployment的绑定只能引用其微版本已声明的alias，不能由业务input新增地址、方法或鉴权头。

每次host.httpRequest由监督器代理到核心HostService.ProxyHttp；核心依据实际RpcDelegation联合验证当前micro.http.request、micro.invoke、micro.runtime.use、原skill.invoke/来源链、绑定限定的secret.use目的与当前secret状态、父egress。secret.use来自已批准凭证使用绑定，不向终端或MPR发放LeaseSecret原值。

业务Call只冻结有序绑定的身份/版本/secret引用，VM只见alias/priority/timeout_ms。冻结不保留被撤销的权限，绑定禁用/删除、secret失效和源链变化逐请求生效；旧Call不能随配置更新静默换key。监督器持有的委托不进入VM，脚本不能自报call_id或访问任意HostService。

同Call的预发送失败或已确认上游429/5xx/认证拒绝可按批准备用alias明确fallback；平台鉴权/绑定状态错误不能被当作上游失败绕过。实际已发但响应不确定时Call保持unknown，禁止后续seq、自动换attempt或重跑non_repeatable脚本，即使VM自行catch或输出JSON也不改变此状态。

## 扩展授权交集

扩展调用同时要求skill.invoke、leaf扩展对象上的extension.invoke，以及逐级真实宿主上的extension.host.use：ordinary匹配实际deployment，micro匹配真实micro_plugin，extension匹配真实extension_plugin。普通根检查当前安装/扩展点审批，不要求无关capability.invoke；微根还需micro.invoke及父micro.runtime.use。

只有实际执行祖先扩展的业务处理器时才检查它的invoke/Skill，宿主边不等于调用父业务Skill。mode=skill另检查固定handler原skill.invoke和目标动作；若发起其他业务capability/Skill调用，仍逐次检查其原动作。完整账户祖先、API Key/源凭证、客户端和根执行委托上限在每个转发处求交，不能用扩展owner、宿主服务账户或MPR本身权限替换调用者。

任一宿主、扩展或handler被停用/撤权时阻止后续执行并传播取消；链摘要和准备句柄都不是授权票据。pure微宿主没有因扩展获得网络或HostService，联网开关与原profile约束始终生效。详见[扩展调用](extensions/README.md)。
