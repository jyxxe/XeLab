# 受限正则提取微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

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

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
