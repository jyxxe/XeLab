# 读取本人存储用量与配额

[对象存储Skill目录](README.md) · [插件配置](../../preset-plugins/object-storage/configuration.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.usage-get@1.0.0`，[定义](usage-get.skill.json)；目标`data.object.usage.get@1.0.0`。响应位于`SkillCallResult.call.output`。

## 输入

输入必须是空JSON对象`{}`，不接收owner_id、分页、祖先账号或全部用户查询参数。完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/usage-get/input.schema.json)。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `owner_id` | UUID字符串 | 内核可信当前subject |
| `logical_bytes` | 非负整数 | 当前可见对象内容长度总和；不含已隐藏旧内容、WAL或其他用户 |
| `reserved_bytes` | 非负整数 | 已准入、尚未提交/释放的用户逻辑字节预留 |
| `object_count` | 非负整数 | 当前可见对象数；零字节对象也计为一个 |
| `quota_bytes` | 非负整数 | 本用户生效逻辑字节上限，默认1073741824 |
| `max_objects` | 非负整数 | 本用户生效对象上限，默认10000 |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/usage-get/output.schema.json)。同一响应读取一致的当前用户计数，不拼接不同时刻的独立统计结果。

## 计量与权限

本接口side_effect=none，不修改对象、配额或业务预留。logical_bytes+reserved_bytes受quota_bytes限制；待创建对象还会预留对象名额，但object_count只报告已经可见的对象数，不把未完成文件伪装成成功对象。

覆盖的用户逻辑预留可只计新旧长度的正差额，而部署实际存储仍需容纳完整新文件与尚未GC的旧文件。删除后logical_bytes可立即减少，物理回收滞后；因此本结果不表示卷剩余空间，也不能用于绕过实际写入准入。

本版按subject独立计量，不提供账户树累计存储查询。API Key使用所属账户同一计数；子账户独立，但共享部署和物理卷总预算不会增加。

响应不暴露部署全卷总量、其他用户数量或路径。调用需当前usage-get Skill与目标能力权限；读取用量不赋予修改quota、部署、卷或其他用户文件的权限，无权为PERMISSION_DENIED。
