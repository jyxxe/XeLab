# 插件清单字段与校验

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 文件与模型

OCI 镜像必须包含 `/xelab/plugin.json`，UTF-8 JSON，最大 1 MiB。完整字段、必填项及约束见 [Manifest](../api/models/Manifest.md) 与 [机器 Schema](../contracts/schemas/Manifest.schema.json)。不允许未知字段、重复 JSON key、非有限数字或宿主绝对路径引用。

安装按如下顺序校验：镜像 digest → 发布者信任与签名 → JSON Schema → plugin_id/version 唯一性 → 平台/host_api_major → 能力 Schema 的有效性及 digest → 依赖范围 → 入口与权限/卷/路由的静态安全检查。

## 字段语义补充

| 字段 | 精确定义 |
|---|---|
| entrypoint | OCI argv；不能由内核拼成 shell 命令；相对配置禁止逃出镜像 |
| capabilities | name/version/schema_digest 唯一；同契约 digest 不一致拒绝安装 |
| config_schema | JSON Schema 2020-12；初始化前验证普通配置；禁止把密钥明文写入 |
| minimum_resources | 启动下限；获批 limits 不得低于此值 |
| requested_rules | 插件代理权限申请；不会自动获得批准或用户权限 |
| volumes | 按别名声明；部署绑定具体授权卷；禁止任意 bind mount |
| routes | 必须位于插件或 UI 命名空间；不能覆盖内核/OAuth/健康路由 |
| data_schema_version | none 表示不持有需迁移数据；否则必须声明迁移策略 |
| upgrade_strategy | 独占写卷默认 recreate；parallel_compatible 还需实际验证兼容 |

清单不可修改已安装版本；修改入口、权限、Schema 或依赖都需新插件版本和 digest。新增权限在升级前单独批准，旧版本审批不能自动扩张。

## 能力 JSON 约束

input/output/stream_item_schema 须为有效 Schema，不接受远程 `$ref` 联网取回；引用仅限制品内受签名覆盖的本地契约包。契约摘要算法见 [契约版本](versioning.md)。大输入输出使用 Blob 资源引用。

## Skill与实例并发

skills[]声明候选Skill，来源安装与定义都固定，安装后不自动启用。business_entry=skill_required时所有业务入口必须经过内核SkillRouter；探针/生命周期不受此约束。concurrency声明整个实例共享的活动处理、排队数量和排队时间，不能按用户或Skill复制额度。

RouteSpec.authentication默认user。仅批准的static/webui GET/HEAD可public，便于加载登录壳；public不开放业务或管理API。HTTP Skill只能绑定已批准业务operation，不能借静态公共路由获取管理身份。

安装接口：[installPlugin](../api/operations/plugins/installPlugin.md)。示例见 [清单样例](../contracts/examples/plugin-manifest.json)。

## 可选UI贡献

ui字段声明PluginUiSpec；与webui桌面壳入口独立。窗口/组件均为隔离iframe，签名静态入口与精确Skill绑定须安装时校验，详见[UI贡献](ui-contributions.md)。

## 微插件运行时声明

普通OCI Manifest可选micro_runtime（[MicroRuntimeDescriptor](../api/models/MicroRuntimeDescriptor.md)）声明MicroRuntimeService的受审版本、引擎和JSON协议。它不是通用业务execute能力，也不自动授权脚本；微插件使用独立[MicroManifest](../api/models/MicroManifest.md)及核心版本/运行绑定，不将源码加入普通容器entrypoint。
