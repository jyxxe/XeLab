# UiSkillBinding

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiSkillBinding.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

构建期声明允许桥接的Skill；安装后解析为精确UUID/digest，不使用latest

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `alias` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9_-]{0,63}$"` | 贡献内唯一别名 |
| `name` | string | 是 | minLength=`1`; maxLength=`160`; pattern=`"^[a-z][a-z0-9.-]+$"` | 插件命名空间内唯一名称，plugin_id 前缀 |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `modes` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`2` | 允许的宿主桥方法；未声明默认全部禁止 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "alias",
    "name",
    "version",
    "modes"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
