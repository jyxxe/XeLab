# 从Blob持久创建或覆盖对象

[对象存储Skill目录](README.md) · [插件接口](../../preset-plugins/object-storage/interfaces.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.object-put@1.0.0`，[定义](object-put.skill.json)；目标`data.object.put@1.0.0`。输入/输出为identity映射，响应位于`SkillCallResult.call.output`。

## 输入

| 字段 | 必填 | 类型与约束 |
|---|---|---|
| `key` | 是 | 本用户逻辑key，NFC UTF-8、1..1024字节，遵循[路径规则](../../preset-plugins/object-storage/interfaces.md) |
| `blob_id` | 是 | UUID；带`x-xelab-resource-kind: blob`标记的获准源Blob |
| `size_bytes` | 是 | 整数0..67108864，必须等于实际完整内容长度 |
| `digest` | 是 | `sha256:`加64位小写十六进制，必须等于实际完整内容摘要 |
| `content_type` | 是 | 长度1..128字符的声明MIME，不含CR/LF；不执行或自动解释内容 |
| `mode` | 是 | `create`或`replace` |
| `expected_revision` | 条件必填 | 正整数；replace必须提供，create禁止提供 |

完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/object-put/input.schema.json)。输入不接收owner、原始文件字节、URL或宿主路径。

create要求key当前没有live对象，生成新object_id；从未使用过的key从revision=1开始，删除后的同key重建使用永久head水位+1，不能重置到1。replace要求当前key存在且revision完全匹配，保持object_id/created_at并将revision加1，content_type可一并更新。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `object` | metadata对象 | 包含id、owner_id、key、revision、size_bytes、digest、content_type、created_at、updated_at，全量字段见[对象模型](../../preset-plugins/object-storage/interfaces.md) |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/object-put/output.schema.json)。仅校验、持久复制和可见metadata原子提交后返回成功；原Blob到期不影响已提交对象。

## 执行与错误

本接口为idempotent，必须使用稳定业务键。先在用户/部署预算预留，再以有界流复制并核实长度和摘要，禁止整文件进入JSON或无界内存；零字节内容合法。完整流程见[存储协议](../../preset-plugins/object-storage/storage.md)。

同键同指纹先重放原结果再进行当前CAS；同键不同指纹或create同key/replace旧revision为STATE_CONFLICT。源Blob无权为PERMISSION_DENIED，不存在/过期为RESOURCE_NOT_FOUND；非法key/摘要/长度为INVALID_ARGUMENT；容量不足为RESOURCE_EXHAUSTED。

超时或响应丢失后使用原键/原Call核实，不以新键盲目覆盖。大量上传通过[异步batch](../batches.md)，每项独立提交，单对象仍最多64MiB。
