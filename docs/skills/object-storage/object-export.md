# 导出指定版本为临时Blob

[对象存储Skill目录](README.md) · [插件接口](../../preset-plugins/object-storage/interfaces.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.object-export@1.0.0`，[定义](object-export.skill.json)；目标`data.object.export@1.0.0`。响应位于`SkillCallResult.call.output`。

## 输入

| 字段 | 必填 | 类型与约束 |
|---|---|---|
| `key` | 是 | 当前subject的完整合法逻辑key |
| `expected_revision` | 是 | 正整数，必须等于当前可见revision |

完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/object-export/input.schema.json)。可先通过[object-stat](object-stat.md)取得revision；旧revision不表示请求历史下载，当前值已改变则拒绝。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `object_id` | UUID字符串 | 本次导出的持久对象ID |
| `revision` | 正整数 | 本次实际验证并读取的revision |
| `blob_id` | UUID字符串 | HostService.WriteBlob创建、归当前主体的临时核心Blob |
| `size_bytes` | 整数 | 完整字节长度，0..67108864 |
| `digest` | 字符串 | 完整内容SHA-256，格式`sha256:`加64位小写十六进制 |
| `content_type` | 字符串 | 持久对象声明MIME |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/object-export/output.schema.json)。输出不含文件内容、公开URL、宿主路径或Host接口未提供的Blob到期时间；默认核心Blob TTL为24h，实际下载/过期状态由核心判定。

## 导出、重试与错误

本接口side_effect=idempotent，因为它创建核心Blob。用稳定业务键保存固定结果；同键同指纹返回原blob_id，不能因为原Blob过期而同键静默重新创建。需要新副本时先核实当前对象，用新键显式再次导出。

插件在CAS检查后pin对应不可变内容，以最多256KiB块流式写核心Blob；完整长度和摘要通过核心校验后才成功。源对象随后被替换或删除不能改写已绑定的内容，也不能撤销合法已导出的副本。

key非法为INVALID_ARGUMENT；对象不存在为RESOURCE_NOT_FOUND；revision失配或同键不同指纹为STATE_CONFLICT；当前对象/Blob动作或插件批准无权为PERMISSION_DENIED；核心Blob容量不足为RESOURCE_EXHAUSTED。

Blob发布与本地结果保存不构成跨存储事务。超时/崩溃后先用原Call/业务键及输出资源账本核实；无法确认的状态保持unknown，不盲目新建。下载仍需核心blob.read，blob_id不是访问授权。大量导出使用[异步batch](../batches.md)。
