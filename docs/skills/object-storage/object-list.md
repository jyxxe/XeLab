# 按前缀分页列出本人对象

[对象存储Skill目录](README.md) · [插件接口](../../preset-plugins/object-storage/interfaces.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.object-list@1.0.0`，[定义](object-list.skill.json)；目标`data.object.list@1.0.0`。响应位于`SkillCallResult.call.output`。

## 输入

| 字段 | 必填 | 类型与约束 |
|---|---|---|
| `prefix` | 否 | 默认空字符串；NFC UTF-8字节前缀，最多1024字节，无控制字符/反斜线，允许`reports/`等尾斜线 |
| `limit` | 否 | 整数1..200，默认50 |
| `cursor` | 否 | 上页返回的不透明非空游标；省略表示首页 |

完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/object-list/input.schema.json)。`{}`列出当前用户全部key的第一页，不查询其他subject或任意宿主目录。prefix区分大小写，不做URL解码或locale比较。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `items` | metadata数组 | 长度0..limit，按key的UTF-8字节升序；每项含完整[对象metadata](../../preset-plugins/object-storage/interfaces.md) |
| `next_cursor` | 字符串或null | 尚有下一页时返回固定有效期游标；遍历结束为null |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/object-list/output.schema.json)。输出只包含当前subject可见对象，不包含历史内容、物理路径或其他用户同名key。

## 分页一致性与错误

本接口side_effect=none，不修改对象或业务配额。每页读取当时可见metadata；分页是keyset向前遍历，不是全库持久快照。并发插入到已越过key之前可能被本次遍历跳过，未读对象可能更新/删除；不会把这种视图称为完整一致快照。

游标绑定subject、prefix、分页参数、数据域/部署代次，默认900秒到期、使用不续期。后续请求沿用原prefix/limit，不借游标变更筛选或用户；游标不能代替当前授权。

格式非法为INVALID_ARGUMENT；绑定不匹配、失效或过期为STATE_CONFLICT。失效后明确重开第一页，不能自动把旧游标当首页返回成功。大量独立列表请求可用[异步batch](../batches.md)，单项仍只取一页。
