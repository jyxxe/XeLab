# 读取本人对象元数据

[对象存储Skill目录](README.md) · [插件接口](../../preset-plugins/object-storage/interfaces.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.object-stat@1.0.0`，[定义](object-stat.skill.json)；目标`data.object.stat@1.0.0`。响应位于`SkillCallResult.call.output`。

## 输入

| 字段 | 必填 | 类型与约束 |
|---|---|---|
| `key` | 是 | 当前subject命名空间中的完整逻辑key，NFC UTF-8、1..1024字节 |

完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/object-stat/input.schema.json)。不接收owner或expected_revision，读取调用当时的当前metadata。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `object` | metadata对象 | id、owner_id、key、revision、size_bytes、digest、content_type、created_at、updated_at，定义见[对象模型](../../preset-plugins/object-storage/interfaces.md) |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/object-stat/output.schema.json)。本接口不读取/返回原始文件字节，不创建临时Blob，不提供历史revision内容。

## 一致性与错误

本接口side_effect=none，无业务写入。结果反映本次读取时刻的当前对象；后续写入可能使revision过时，修改、导出或删除必须携带刚读取的revision并接受CAS校验。

key非法为INVALID_ARGUMENT；当前用户下不存在为RESOURCE_NOT_FOUND。另一用户拥有相同key不会使本接口成功，也不通过错误信息暴露其对象是否存在。当前Skill/能力或来源无权为PERMISSION_DENIED。

读取metadata不等于获得下载权限。获取字节需另调[object-export](object-export.md)，其创建Blob及核心下载动作分别重新鉴权。多个独立stat可用[异步batch](../batches.md)。
