# Markdown 转 HTML 微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `convert.markdown-html`，把有界 Markdown 文本转换成经过清理的 HTML 片段。能力 ID 仅作规划索引，不代表已经可用的服务或 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md)、受审 Markdown 解析器和 HTML 清理规则/实现。
- 支持静态文字、段落、标题、列表、引用、代码与链接的明确子集；不执行脚本、模板、插件扩展或远程资源加载。
- 原始 HTML 按清理规则处理；不得保留脚本、事件属性、危险 URL 协议或能突破页面隔离的内容。
- 结果只是数据，不授予 WebUI 的 `trustHTML`、同源脚本或其他渲染特权；消费方仍使用自己的隔离和清理策略。

## 运行与启用门禁

微插件复用 MPR，没有独立 OCI。规划配置为 `pure-json-v1`、`side_effect=none`；输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

现有 MPR 不包含已承诺的 Markdown/HTML 库或浏览器 DOM。两类实现必须受审打包至不可变制品，或通过明确 Runtime 扩展补齐；禁止在脚本中临时安装依赖。清理器与解析器的版本及允许规则必须可审核后才可发布。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，不是可直接安装的 MicroManifest；未来须展开成无 `$ref` 的受限业务 Schema，再通过运行库与预算门禁。

核心仅在未来微部署创建时生成初始禁用候选 Skill，固定版本、微部署和摘要；不发布静态 `kind=capability` Skill 或裸服务。多个文档使用[异步 batch](../../skills/batches.md)。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
