# 微插件动态Skill

[Skill目录](../README.md) · [核心微对象](../../architecture/micro-plugins.md) · [MPR预设](../../preset-plugins/micro-plugin-runtime/README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现脚本执行。

MPR预设不发布通用execute业务Skill。发布微版本后创建MicroDeployment，核心生成candidate_skill，再通过registerSkill显式注册/启用；不能由脚本向注册表自报接口。

target.kind=micro固定micro_version_id、micro_deployment_id、package_digest和runtime plugin_id，CreateSkill.installation_id必须与微版本固定runtime_installation_id一致。业务输入输出精确等于MicroManifest，调用方只发业务JSON，不可覆盖绑定或传即时源码/argv/env。

name为运行时plugin_id加.micro.<micro_deployment UUID>.run；version为MicroManifest.version。改变微制品、原limits或父运行时绑定使用新微版本或微部署，不能原地修改已注册定义。默认mapping=identity，运行时控制RPC封装由核心添加，脚本只看到业务input。

MPR 1.2.0的微部署settings与这些固定定义分开：`resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）为默认值，部署全部worker在核心适配器配置的逐微cgroup内合计受限；默认并发1、硬上限4共享预算，JS heap默认16MiB仍为每VM子上限。没有每微OCI，也不向MPR委派宿主cgroup权限。

`PUT /api/platform/v1/micro-deployments/{id}/settings`接收`{resources:{cpu_millis,memory_bytes},network_enabled}`，须带If-Match及Idempotency-Key。仅enabled=false、phase=disabled且执行已排空时可写，其余运行状态返回STATE_CONFLICT；管理流程为停用、等待排空、配置、重新启用。更新generation/revision并使旧句柄失效，原limits、版本、父绑定及注册Skill定义不变。

network_enabled默认false，Prepare/Ready和pure执行仍可继续；controlled-http-v1 Invoke在false时于worker创建及发送前返回PERMISSION_DENIED，violations.field=network_enabled，不消耗sequence或触发fallback。true仍须正确profile、权限、固定HttpBinding和平台策略，不赋予任意fetch/socket；每次ProxyHttp检查开关、enabled及generation。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、缓存256MiB；默认最多4个不同微部署同时活跃，总活动槽4只是上限。batch仍按同一Skill和当前代次准入，不扩张或借用内核4C/16GiB预留。

调用需skill.invoke、具体micro_plugin UUID上的micro.invoke、父普通deployment上的micro.runtime.use及完整当前来源链。仅有运行时管理权不能调用任意微插件，Key可精确限制微ID和Skill UUID；不会退回通用capability.invoke绕过。

[候选定义示例](../../preset-plugins/micro-plugin-runtime/examples/echo.skill.json)、[注册请求](../../preset-plugins/micro-plugin-runtime/examples/register-skill.json)、[调用请求](../../preset-plugins/micro-plugin-runtime/examples/invoke.json)为可校验静态示例，UUID需替换为核心返回值，动态定义不被伪称OCI静态签名内容。

批处理复用[核心异步batch](../batches.md)，不额外开脚本队列API；单次时限仍受微限额。v1不支持流式、HostService子调用或AI模型别名适配。
