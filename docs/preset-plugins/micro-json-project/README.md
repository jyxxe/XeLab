# JSON 字段投影微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

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
