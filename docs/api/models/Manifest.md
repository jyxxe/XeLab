# Manifest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Manifest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

OCI 镜像内 /xelab/plugin.json；未知字段拒绝

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `manifest_version` | string | 是 | enum=`["1"]` | 清单版本 |
| `plugin_id` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$"` | 插件稳定 ID；不含版本 |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `publisher_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `host_api_major` | integer | 是 | minimum=`1`; maximum=`1` | 宿主 API 主版本 |
| `platforms` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`2` | 可运行平台 |
| `entrypoint` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`64` | OCI argv |
| `capabilities` | array&lt;[CapabilityContract](CapabilityContract.md)&gt; | 是 | minItems=`0`; maxItems=`64` | 提供的契约 |
| `dependencies` | array&lt;[DependencySpec](DependencySpec.md)&gt; | 是 | minItems=`0`; maxItems=`64` | 依赖 |
| `requested_rules` | array&lt;[PermissionRequest](PermissionRequest.md)&gt; | 是 | minItems=`0`; maxItems=`256` | 插件宿主权限申请 |
| `minimum_resources` | [ResourceLimits](ResourceLimits.md) | 是 | 见类型定义 | 实例获批上限；磁盘限额由配额存储实施 |
| `health` | [HealthCheckSpec](HealthCheckSpec.md) | 是 | 见类型定义 | 插件启动、存活和就绪检查 |
| `volumes` | array&lt;[VolumeMountSpec](VolumeMountSpec.md)&gt; | 是 | minItems=`0`; maxItems=`16` | 声明卷 |
| `routes` | array&lt;[RouteSpec](RouteSpec.md)&gt; | 是 | minItems=`0`; maxItems=`32` | HTTP 路由 |
| `webui` | [WebUiSpec](WebUiSpec.md) | 否 | 见类型定义 | 浏览器界面元数据 |
| `config_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `data_schema_version` | string | 是 | minLength=`1`; maxLength=`64` | 有状态数据格式；无状态填 none |
| `upgrade_strategy` | string | 是 | enum=`["recreate", "parallel_compatible"]` | recreate 适用于独占可写数据 |
| `event_topics` | array&lt;[EventTopicSpec](EventTopicSpec.md)&gt; | 否 | minItems=`0`; maxItems=`32` | 声明发布的事件主题契约 |
| `skills` | array&lt;JSON&gt; | 否 | minItems=`0`; maxItems=`64` | 制品签名覆盖的候选 Skill 定义；安装后不自动启用 |
| `business_entry` | string | 否 | default=`"direct"`; enum=`["direct", "skill_required"]` | 业务必须经 Skill 时，裸调用不得绕过 |
| `concurrency` | object | 否 | 见类型定义 | 实例业务共享并发池；探针和生命周期独立 |
| `ui` | [PluginUiSpec](PluginUiSpec.md) | 否 | 见类型定义 | 可选UI贡献；独立于webui字段，允许仅窗口、仅组件、两者或均不提供 |
| `micro_runtime` | [MicroRuntimeDescriptor](MicroRuntimeDescriptor.md) | 否 | 见类型定义 | 普通OCI插件可提供的受审微执行适配协议声明，不是业务能力 |
| `extension_points` | array&lt;[ExtensionPoint](ExtensionPoint.md)&gt; | 否 | minItems=`0`; maxItems=`8` | 可选宿主点；省略=无扩展，不回写旧清单或重算镜像/包摘要 |
| `extension_host` | [ExtensionHostDescriptor](ExtensionHostDescriptor.md) | 否 | 见类型定义 | 物理OCI根对ExtensionHostService的明确能力声明；缺省不支持，不因插件类型而自动推断 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "manifest_version",
    "plugin_id",
    "version",
    "publisher_id",
    "host_api_major",
    "platforms",
    "entrypoint",
    "capabilities",
    "dependencies",
    "requested_rules",
    "minimum_resources",
    "health",
    "volumes",
    "routes",
    "config_schema",
    "data_schema_version",
    "upgrade_strategy"
  ],
  "allOf": [
    {
      "if": {
        "required": [
          "micro_runtime"
        ]
      },
      "then": {
        "properties": {
          "plugin_id": {
            "maxLength": 113
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "extension_points": {
            "minItems": 1
          }
        },
        "required": [
          "extension_points"
        ]
      },
      "then": {
        "required": [
          "extension_host"
        ],
        "properties": {
          "extension_host": {
            "properties": {
              "host_kinds": {
                "contains": {
                  "const": "ordinary"
                }
              }
            }
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
