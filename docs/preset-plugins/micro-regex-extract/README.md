# 受限正则提取微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `text.regex-extract`，在受控输入上提取匹配及捕获组。能力 ID 只用于规划索引，不代表已部署服务或注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 与受审的受限正则引擎。
- 只接受对固定语法子集证明线性时间的受限引擎；普通原生 JavaScript `RegExp` 或回溯引擎加超时强杀不能作为等价实现。
- 不支持动态代码、替换表达式、宿主回调、回溯引用或未审查的高级模式；不把正则文本当作可执行代码。
- 文本和模式均受限；用户不能调整资源上限到平台硬上限之外。

## 运行与启用门禁

微插件复用 MPR，无独立 OCI。规划为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

现有 MPR 的 ECMAScript JSON 不保证线性正则库已可用。必须在不可变制品中受审打包固定线性引擎，或先完成明确 Runtime 扩展；匹配和模式编译仍受总期限并须验证强制终止，无法满足时禁止发布。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，不构成可直接安装的 MicroManifest。未来发布前须展开为无 `$ref` 的受限业务 Schema，再通过运行库、Unicode 索引和预算门禁。

核心在未来微部署创建时才生成初始禁用候选 Skill，绑定微版本、微部署与摘要；不发布静态 `kind=capability` Skill 或裸接口。多份提取任务采用[异步 batch](../../skills/batches.md)，不能扩大单项期限。
