# CreateExtensionDeployment

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateExtensionDeployment.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建disabled扩展部署；锁图校验typed宿主/点/摘要/handler与完整DAG，固定精确链后生成候选Skill，不自动注册或启用

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `extension_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `host` | [ExtensionHostRef](ExtensionHostRef.md) | 是 | 见类型定义 | 且仅一个typed宿主；客户端ID仅是待验证目标，服务端锁图读取并核对真实安装、版本、包和当前权限 |
| `config` | object | 是 | 见类型定义 | 固定扩展配置<=64KiB/深度32，须满足宿主点config_schema；变更须新部署 |
| `priority` | integer | 否 | minimum=`0`; maximum=`1000`; default=`100` | 同一点按priority升序、UUID排序，顺序随Call冻结 |
| `timeout_ms` | integer | 否 | minimum=`100`; maximum=`30000`; default=`2000` | 固定候选Skill总期限，不能超过点、物理根或调用剩余期限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "extension_version_id",
    "host",
    "config"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
