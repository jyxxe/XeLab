# ExtensionDeployment

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ExtensionDeployment.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

固定扩展版本/宿主/配置/契约及候选Skill；不创建OCI，host链由核心解析，不接受调用方改写物理地址

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `enabled` | boolean | 是 | 见类型定义 |  |
| `generation` | integer | 是 | minimum=`1`; maximum=`2147483647` | 期望启停或资源/联网设置发生实质变更时递增；旧Prepare句柄和迟到执行不得覆盖新代次 |
| `reason_codes` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 当前阻塞原因 |
| `candidate_skill` | [SkillDefinition](SkillDefinition.md) | 是 | 见类型定义 | 不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md |
| `extension_plugin_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `extension_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `host` | [ExtensionHostRef](ExtensionHostRef.md) | 是 | 见类型定义 | 且仅一个typed宿主；客户端ID仅是待验证目标，服务端锁图读取并核对真实安装、版本、包和当前权限 |
| `host_chain` | array&lt;[ExtensionHostRef](ExtensionHostRef.md)&gt; | 是 | minItems=`1`; maxItems=`16` |  |
| `host_chain_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 核心规范ExtensionIdentityClosure的稳定SHA，逻辑宿主链不含实例/部署运行generation或可变微资源；不等同prepared_handle |
| `root_installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `root_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `config` | object | 是 | 见类型定义 | 固定扩展配置<=64KiB/深度32，须满足宿主点config_schema；变更须新部署 |
| `priority` | integer | 是 | minimum=`0`; maximum=`1000`; default=`100` | 同一点按priority升序、UUID排序，顺序随Call冻结 |
| `timeout_ms` | integer | 是 | minimum=`100`; maximum=`30000`; default=`2000` | 固定候选Skill总期限，不能超过点、物理根或调用剩余期限 |
| `phase` | string | 是 | enum=`["disabled", "preparing", "ready", "waiting_host", "draining", "failed", "archived"]` | 宿主先就绪；失效撤路由，停用逆序排空，真实卸载后disabled/archived |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "owner_id",
    "enabled",
    "generation",
    "reason_codes",
    "candidate_skill",
    "extension_plugin_id",
    "extension_version_id",
    "package_digest",
    "host",
    "host_chain",
    "host_chain_digest",
    "root_installation_id",
    "root_deployment_id",
    "config",
    "priority",
    "timeout_ms",
    "phase"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
