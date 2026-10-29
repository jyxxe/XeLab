# JSON 校验微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `json.validate`，用于在数据进入后续流程前，根据受限 Schema 校验 JSON 并输出结构化错误。能力 ID 只用于规划索引，不代表一个可直接调用的服务或已注册 Skill。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 及受审的受限 Schema 校验实现。
- 只接受 [MicroBusinessSchema](../../api/models/MicroBusinessSchema.md) 的关键词集合；每份 Schema 至多 64KiB、256 节点、深度 16。布尔 Schema 也受支持。
- 不执行远程或本地 `$ref`、任意正则、自定义格式处理器、用户函数及默认值填充；不把此能力当作完整 JSON Schema 执行器。
- Schema 本身不合法属于调用失败；文档不满足合法 Schema 属于正常业务结果 `valid=false`。

## 运行与启用门禁

这是复用 MPR 的微插件，不为它建立独立 OCI 容器。规划执行配置为 `pure-json-v1`、`side_effect=none`：输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s，硬上限 30s；部署只能进一步收紧限制。

现有 MPR 的 ECMAScript JSON 支持不代表已提供 Schema 校验库。实现必须将受审校验器打包到不可变制品，或先完成明确的 Runtime 扩展；在预算内完成 Schema 构建、文档遍历与错误收集的验证后才能发布。

本目录的 raw 业务契约允许引用[规划共享 Schema](../planned/shared.schema.json)，属于设计草案，不是可直接安装的 MicroManifest。未来发布前必须展开为无 `$ref` 的受限业务 Schema，再通过预算与运行库门禁。

未来创建微部署后才由核心生成候选 Skill，初始禁用。候选必须绑定微版本、微部署和制品摘要；本规划不发布静态 `kind=capability` Skill，也不暴露绕过核心的裸服务。多份文档使用[异步 batch](../../skills/batches.md)，每项仍执行独立校验。
