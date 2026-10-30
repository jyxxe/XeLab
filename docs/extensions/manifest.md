# 扩展清单与不可变制品

[扩展目录](README.md) · [宿主与扩展点](hosts.md) · [机器模型](../api/models/README.md)

> 规格版本：2.8.0；状态：待实现的规范。清单字段及边界以OpenAPI对应模型为结构来源。

扩展有独立的ExtensionPlugin身份、不可变版本及部署。发布版本时保存ExtensionManifest、贡献原始字节摘要、包摘要和当前所有者来源；发布记录与宿主安装来源分别核实，不能把扩展当作宿主OCI签名的一部分。

## ExtensionManifest

| 字段 | 定义 |
|---|---|
| `host_kind` | 声明目标宿主为ordinary、micro或extension；部署时必须与固定宿主引用类型一致 |
| `point_id` | 要扩展的精确扩展点，不允许输入任意方法名或宿主内部路径 |
| `contract` | 该扩展点的精确能力契约名、版本和Schema摘要；名称相同不代表摘要可互换 |
| `mode` | data或skill，必须与宿主点一致，不以运行输入切换 |
| `handler_skill_id` | 仅skill模式声明；固定已注册Skill UUID，不是可动态解析的名称 |
| `handler_definition_digest` | 仅skill模式声明；固定处理器不可变定义摘要 |
| `extension_points` | 可选，声明本扩展愿意接受下一层扩展的点；不自动继承上层全部扩展点 |

身份格式、版本、说明、执行限制及其他必填项见机器模型。data模式不得携带处理器字段，skill模式必须同时给出处理器UUID与定义摘要；未知字段拒绝。清单中的扩展点或处理器变化必须发布新版本，不能在已有包上换内容。

## contribution.json

扩展内容使用`contribution.json`，原始UTF-8最多256KiB，必须是一个完整的严格JSON值。拒绝BOM、重复key、非有限数字、不完整JSON、多份串联JSON及任何以压缩或路径引用绕过字节限额的输入。

内容必须符合固定宿主扩展点的contribution_schema。data由宿主已实现的数据适配器解释；skill可把贡献作为该点规定的数据配置，但它不携带任意目标、委托或可执行代码。贡献中的URL、blob_id、owner_id及“命令”字样都只是数据，不产生网络、文件或身份权限。

宿主点的input、output、contribution及config Schema采用受限MicroBusinessSchema：每份最多64KiB、256节点、深度16，禁止远程引用、正则及自定义format等未支持关键字。业务值还受点、扩展及实际执行根的字节/深度预算共同约束；Schema通过不证明代码执行或外部副作用安全。

贡献不提供`.js`、`.so`、Python包或可执行entrypoint。处理器代码由其原普通或微插件的独立制品负责，不因被引用为扩展处理器而变成可以直接加载的宿主库。

## 摘要与版本

source_digest固定contribution.json的原始字节；格式化JSON也会改变该摘要。package_digest按机器契约规定的规范包摘要算法，将清单与贡献来源摘要绑定；部署和候选Skill不得使用无对应实际字节的占位摘要。

扩展点的contract摘要必须与宿主发布的点契约一致。输入、输出、贡献、配置、模式、副作用上限及期限都属于兼容性约束；仅有相同point_id或SemVer范围不足以证明可加载。

点契约沿用平台四文件算法：input.schema.json、output.schema.json、stream.schema.json、semantics.json，按该顺序计算每文件SHA256，再对`文件名 + 空格 + 小写摘要 + 换行`的清单求SHA256。stream固定为null类型；semantics须包含point_id、mode、side_effect、maximum_timeout_ms、maximum_input_bytes、maximum_output_bytes、maximum_contributions、contribution_schema和config_schema，与点声明逐项相等。发布前校验Schema引用闭包，不能仅改点配置而沿用旧摘要。

版本清单不保存尚未选择的宿主部署或未生成的候选Skill。创建扩展部署时才解析真实宿主及整条链、固定config/limits并生成候选；不把可变实例地址写进版本制品。

## 与已有清单兼容

普通Manifest可选声明extension_points及extension_host协议；MicroManifest可选声明extension_points；ExtensionManifest也可声明自己的extension_points。省略表示无扩展点或无相应加载能力，不为旧包回写默认空数组或重新计算其摘要。

已有微源码仍按原包、profile和MPR绑定解释。支持只读扩展数据的微版本必须明确声明扩展点，并由声明相应宿主协议的MPR1.3承载；升级MPR不自动给旧微VM增加扩展、网络或宿主模块入口。

宿主升级、更换扩展点契约、扩展内容或固定处理器都需按不可变版本和显式部署迁移处理。旧任务保存的包与宿主链不得按同名最新制品替换。完整样例见[扩展示例](examples/README.md)。
