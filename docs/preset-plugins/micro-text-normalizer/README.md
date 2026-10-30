# 文本规范化微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `text.normalize`，用于在搜索、切块与 Embedding 前统一 Unicode、换行和指定空白规则。能力 ID 只用于规划分类，不代表可用服务或已注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 以及明确版本的 Unicode 规范化实现。
- 只执行声明的文本规则；不执行用户正则、代码、语言翻译或语义改写。
- 默认只归一换行；兼容性归一可能改变字符含义，只有显式选择 NFKC/NFKD 时执行。
- 不将规范化结果当作身份凭证比较、路径校验或 HTML 清理的替代方案。

## 运行与启用门禁

复用 MPR，无独立 OCI；规划为 `pure-json-v1`、`side_effect=none`。输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

MPR 目前保证 ECMAScript JSON，不自动承诺特定 Unicode 表和规范化 API。实现必须证明目标引擎提供固定语义，或在不可变制品内受审打包库；补齐前不得宣称所有规范化形式均可用。制品记录 Unicode 版本，升级随版本与摘要变化。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，并非可直接安装的 MicroManifest。未来发布前须展开为无 `$ref` 的受限业务 Schema，再通过运行库和预算门禁。

未来创建微部署后核心才产生初始禁用候选 Skill，绑定微版本、微部署和制品摘要；无静态 `kind=capability` Skill 或裸服务。多文本调用使用[异步 batch](../../skills/batches.md)。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
