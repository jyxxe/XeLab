# CSV / JSON 转换微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `convert.csv-json`、`convert.json-csv`，用于有界表格文本与 JSON 记录间转换。能力 ID 只标识规划目录，不代表已安装服务或固定 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 及受审的 CSV 解析/序列化库。
- 采用 RFC 4180 的双引号、引号转义和记录规则，并明确分隔符、标题行、空值及行列上限。
- CSV 字段不自动推断数字、日期、布尔值，也不执行单元格公式；JSON 转 CSV 的支持值仅限标量。
- 不读写宿主文件；输入和输出均是 JSON 内的文本或记录数据。大量文件转换需要另行有界任务设计。

## 运行与启用门禁

复用 MPR，无独立 OCI 容器；规划使用 `pure-json-v1`、`side_effect=none`。输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

MPR 当前只保证 ECMAScript JSON，不承诺 CSV 库已可用。解析器须受审打包进不可变制品，或先完成明确 Runtime 扩展；需验证带换行字段、引号边界、行列攻击及输出膨胀后才可发布。

raw 契约允许引用[规划共享 Schema](../planned/shared.schema.json)，不代表可直接安装的 MicroManifest；未来发布前必须展开成无 `$ref` 的受限业务 Schema，并通过运行库和预算门禁。

候选 Skill 仅在未来创建微部署时由核心生成并初始禁用，绑定微版本、微部署及制品摘要；不发布静态 `kind=capability` Skill 或裸服务。多个转换任务使用[异步 batch](../../skills/batches.md)，不能借 batch 解除单项限制。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
