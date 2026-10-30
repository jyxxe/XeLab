# 流程条件微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `flow.condition`，根据结构化输入和声明条件返回判断结果，供上层流程选择分支。它不负责调度或实际执行分支；能力 ID 只是规划索引。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 和受审的有界声明条件解释器。
- 与[流程过滤微插件](../micro-flow-filter/README.md) 共用条件 AST；字段地址使用 RFC 6901 JSON Pointer。
- 不使用知识库 `match` / `topk` 查询语言，不接受 JavaScript、SQL、正则、用户函数或动态代码。
- 仅比较本次 JSON 数据；无文件、网络、密钥、当前时间或跨调用状态访问。

## 运行与启用门禁

复用 MPR，无独立 OCI。规划执行配置为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。条件 AST 另限深度 16、总节点 256。

MPR 现有 ECMAScript JSON 不自动包含该条件语言。解释器和路径解析须在不可变制品中受审打包，验证类型比较、缺失值和短路规则后才能发布；需要新的 Runtime 能力时必须显式扩展，不能使用 `eval` 补齐。

raw 契约通过[规划共享 Schema](../planned/shared.schema.json) 表达复用 AST，不是可直接安装的 MicroManifest；未来须先将有界 AST 展开为无 `$ref` 的受限业务 Schema，验证 Schema 预算及解释器门禁，无法通过时保持未部署。

未来创建微部署时核心才产生初始禁用的候选 Skill，固定微版本、微部署与摘要；无静态 `kind=capability` Skill 或裸服务。多次判断使用[异步 batch](../../skills/batches.md)。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
