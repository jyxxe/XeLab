# ExtensionHostRef

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ExtensionHostRef.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

且仅一个typed宿主；客户端ID仅是待验证目标，服务端锁图读取并核对真实安装、版本、包和当前权限

## 字段

类型：[OrdinaryExtensionHostRef](OrdinaryExtensionHostRef.md) / [MicroExtensionHostRef](MicroExtensionHostRef.md) / [NestedExtensionHostRef](NestedExtensionHostRef.md)。且仅一个typed宿主；客户端ID仅是待验证目标，服务端锁图读取并核对真实安装、版本、包和当前权限

## 对象及跨字段约束

```json
{
  "oneOf": [
    {
      "$ref": "#/components/schemas/OrdinaryExtensionHostRef"
    },
    {
      "$ref": "#/components/schemas/MicroExtensionHostRef"
    },
    {
      "$ref": "#/components/schemas/NestedExtensionHostRef"
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
