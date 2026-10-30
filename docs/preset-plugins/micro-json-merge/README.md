# JSON 合并微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `json.merge`，将多个 JSON 对象按明确规则合并，适用于配置、任务参数和结构化中间数据的组合。能力 ID 只用于规划索引，不代表可直接调用的服务或已注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 及受审的纯 JSON 遍历/复制实现。
- 支持浅合并、深合并，以及数组替换或连接；不提供任意冲突回调、不执行代码、不合并宿主对象。
- 输入必须是 JSON 对象，不是类实例；对象键没有原型、访问器或可执行语义。
- 合并不写文件、不修改输入、不保存跨调用状态。

## 运行与启用门禁

这是复用 MPR 的微插件，无独立 OCI 容器。规划执行配置为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

现有 ECMAScript JSON 能力不自动证明合并算法能处理深层对象、恶意键名与数组膨胀。实现必须在不可变制品中受审，按输出增长预检并验证资源失败路径，不能假定依赖任意第三方模块。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，只是设计草案；未来必须展开为无 `$ref` 的受限业务 Schema 并通过运行库和预算门禁，不能直接作为 MicroManifest 安装。

候选 Skill 仅在未来创建微部署后由核心生成，初始禁用，绑定实际微版本、微部署与摘要；不提供静态 `kind=capability` Skill 或绕开核心的裸接口。多次合并使用[异步 batch](../../skills/batches.md)。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
