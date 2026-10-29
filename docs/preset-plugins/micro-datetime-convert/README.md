# 日期时间转换微插件

[预设目录](../README.md) · [接口](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned，待实现；业务契约草案：0.1.0。

能力索引为 `datetime.convert`，用于严格解析日期时间并输出指定时区的统一格式。能力 ID 只表示规划能力，不代表已注册 Skill 或可直接调用服务。

## 依赖与范围

- 依赖[公共契约](../contracts/README.md)、[Micro Plugin Runtime](../micro-plugin-runtime/README.md) 和受审日期时间实现。
- 基础范围为 UTC 与固定数值偏移；IANA 区域时区只有在制品包含明确版本的受审 tzdb 后才能启用。
- 不读取宿主本地时区、不根据当前时间猜测缺失日期，不进行模糊自然语言解析。
- 无网络查询和当前时钟依赖；相同输入和固定制品必须得到相同结果。

## 运行与启用门禁

微插件复用 MPR，没有独立 OCI；规划执行配置为 `pure-json-v1`、`side_effect=none`。输入和输出各最多 1MiB、JSON 深度 64、heap 16MiB、stack 1MiB、总期限默认 2s、硬上限 30s。

当前 MPR 的 ECMAScript JSON 不保证日期库、IANA 时区数据库或特定日期 API。基础实现须受审打包进不可变制品；IANA 支持还须固定 tzdb 版本和合法本地时间的歧义规则，或先完成显式 Runtime 扩展。不能假定宿主系统 tzdata 可见。

raw 契约可引用[规划共享 Schema](../planned/shared.schema.json)，不是可直接安装的 MicroManifest。未来发布前必须展开为无 `$ref` 的受限业务 Schema，再通过运行库和预算门禁。

候选 Skill 仅在未来微部署创建时由核心生成，初始禁用，绑定微版本、微部署与摘要；无静态 `kind=capability` Skill 或裸服务。多项转换使用[异步 batch](../../skills/batches.md)。
