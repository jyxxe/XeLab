# JSON 字段投影微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `json.project`，用于按字段路径提取、重命名和投影 JSON 数据。能力 ID 只标识规划分类，不代表已安装的接口或 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 和受审的 JSON Pointer 解析及纯 JSON 复制实现。
- 源与目标字段路径使用 RFC 6901 JSON Pointer；不支持点表达式、通配符、JSONPath、`eval` 或宿主属性访问。
- 本接口只投影字段；条件转换使用 flow 系列，任意表达式计算不在范围内。
- 输出从新的 JSON 对象构建，不访问原型链或宿主对象，也不修改输入。

## 运行与启用门禁

复用 MPR，不创建独立 OCI 容器。规划为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

MPR 的 ECMAScript JSON 支持不自动提供本规划的安全路径实现。必须在不可变制品内打包并审核路径和复制逻辑；验证空路径、转义、缺失值、数组索引和恶意键名行为后才能发布。

raw 业务契约可引用[规划共享 Schema](../planned/shared.schema.json)，不构成可直接安装的 MicroManifest；未来发布前必须展开为无 `$ref` 的受限业务 Schema，并通过运行库及预算门禁。

创建微部署后核心才生成初始禁用的候选 Skill，并固定微版本、微部署与制品摘要。没有静态 `kind=capability` Skill 或独立裸服务。多项投影通过[异步 batch](../../skills/batches.md)，单项预算不扩大。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
