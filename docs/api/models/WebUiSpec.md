# WebUiSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/WebUiSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

浏览器界面元数据

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `base_path` | string | 是 | minLength=`1`; maxLength=`512` | 平台内部转发路径 |
| `entry_path` | string | 是 | minLength=`1`; maxLength=`256` | 容器内 HTTP 页面路径 |
| `management_api_major` | integer | 是 | minimum=`1`; maximum=`1` | 兼容管理 API 主版本，当前为 1 |
| `isolation` | string | 是 | enum=`["dedicated_origin", "sandbox"]` | 非互信 UI 必须独立源或 sandbox |
| `origin` | string / null | 是 | 见类型定义 | null 请求平台按固定 deployment_id 分配独立 HTTPS origin；非空只能精确匹配该分配值。签名制品不预知新部署 UUID，也不得指定可信内核域或其他插件域。 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "base_path",
    "entry_path",
    "management_api_major",
    "isolation",
    "origin"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
