# 微插件动态Skill

[Skill目录](../README.md) · [核心微对象](../../architecture/micro-plugins.md) · [MPR预设](../../preset-plugins/micro-plugin-runtime/README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现脚本执行。

MPR预设不发布通用execute业务Skill。发布微版本后创建MicroDeployment，核心生成candidate_skill，再通过registerSkill显式注册/启用；不能由脚本向注册表自报接口。

target.kind=micro固定micro_version_id、micro_deployment_id、package_digest和runtime plugin_id，CreateSkill.installation_id必须与微版本固定runtime_installation_id一致。业务输入输出精确等于MicroManifest，调用方只发业务JSON，不可覆盖绑定或传即时源码/argv/env。

name为运行时plugin_id加.micro.<micro_deployment UUID>.run；version为MicroManifest.version。改变微制品或运行绑定/预算使用新微版本或微部署，不能原地修改已注册定义。默认mapping=identity，运行时控制RPC封装由核心添加，脚本只看到业务input。

调用需skill.invoke、具体micro_plugin UUID上的micro.invoke、父普通deployment上的micro.runtime.use及完整当前来源链。仅有运行时管理权不能调用任意微插件，Key可精确限制微ID和Skill UUID；不会退回通用capability.invoke绕过。

[候选定义示例](../../preset-plugins/micro-plugin-runtime/examples/echo.skill.json)、[注册请求](../../preset-plugins/micro-plugin-runtime/examples/register-skill.json)、[调用请求](../../preset-plugins/micro-plugin-runtime/examples/invoke.json)为可校验静态示例，UUID需替换为核心返回值，动态定义不被伪称OCI静态签名内容。

批处理复用[核心异步batch](../batches.md)，不额外开脚本队列API；单次时限仍受微限额。v1不支持流式、HostService子调用或AI模型别名适配。
