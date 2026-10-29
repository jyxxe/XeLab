# 流程字段映射微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

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
