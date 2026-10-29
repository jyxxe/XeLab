# MicroHttpEndpoint

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroHttpEndpoint.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

由微版本固定的HTTPS API模板；业务输入不能选择URL/headers

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `alias` | string | 是 | minLength=`1`; maxLength=`32`; pattern=`"^[a-z][a-z0-9-]{0,31}$"` | 不可变微清单中的endpoint别名 |
| `origin` | string | 是 | format=`"uri"`; minLength=`1`; maxLength=`512`; pattern=`"^https://"` | 精确HTTPS origin，仅443；无路径/用户信息/query/fragment，解析后验证公共目标IP |
| `path` | string | 是 | minLength=`1`; maxLength=`256`; pattern=`"^/[A-Za-z0-9_./-]*$"` | 固定绝对API路径；拒绝dot/编码逃逸/query/fragment |
| `method` | string | 是 | enum=`["GET", "POST"]` |  |
| `auth_header` | string | 是 | enum=`["Authorization", "X-Subscription-Token", "X-API-Key"]` |  |
| `auth_scheme` | string | 是 | enum=`["raw", "bearer"]` |  |
| `payload_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "alias",
    "origin",
    "path",
    "method",
    "auth_header",
    "auth_scheme",
    "payload_schema"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
