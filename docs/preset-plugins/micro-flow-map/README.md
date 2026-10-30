# 流程字段映射微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `flow.map`，把本次输入映射成目标对象，支持字段读取、常量、缺失默认值和简单类型转换。能力 ID 只标识规划能力，不代表直接可调用服务或已注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 及受审的 JSON Pointer/JSON 映射实现。
- 路径使用 RFC 6901 JSON Pointer；每条映射显式选择字段或字面常量，不执行字符串表达式。
- 不提供循环、函数调用、模板执行、数学表达式、网络请求或任意宿主代码；复杂流程由上层组合多个有界能力。
- 只输出新的 JSON 对象，不修改输入或持久状态；目标对象不得访问宿主原型链。

## 运行与启用门禁

复用 MPR，没有独立 OCI。规划使用 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

当前 MPR 的 ECMAScript JSON 不代表已包含本规划的映射器。路径、默认值与严格类型转换须在不可变制品中受审打包，或通过明确 Runtime 扩展提供；禁止用 `eval`、`Function` 或宿主对象赋值实现不受控扩展。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，并非可直接安装的 MicroManifest。未来发布前须展开为无 `$ref` 的受限业务 Schema，验证运行库和 Schema/执行预算后才能部署。

未来创建微部署后核心才产生初始禁用候选 Skill，固定微版本、微部署和摘要；不提供静态 `kind=capability` Skill 或裸服务。多对象映射采用[异步 batch](../../skills/batches.md)，每项保持原预算。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
