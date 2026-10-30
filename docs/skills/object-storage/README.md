# 对象存储业务Skill

[文档总目录](../../README.md) · [插件设计](../../preset-plugins/object-storage/README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

所有Skill版本1.0.0，固定指向`org.xelab.object-storage`普通OCI插件的1.0.0能力契约；由核心转发，不向用户开放插件直连文件端口。

| Skill后缀 | 用途 | 目标能力 |
|---|---|---|
| [object-put](object-put.md) | 从核心Blob创建或CAS覆盖持久对象 | `data.object.put` |
| [object-list](object-list.md) | 按逻辑前缀分页列出本人对象 | `data.object.list` |
| [object-stat](object-stat.md) | 读取当前对象metadata及revision | `data.object.stat` |
| [object-export](object-export.md) | 导出指定revision为临时核心Blob | `data.object.export` |
| [object-delete](object-delete.md) | CAS删除一个对象 | `data.object.delete` |
| [usage-get](usage-get.md) | 读取本人逻辑容量、预留和配额 | `data.object.usage.get` |

完整名称为`org.xelab.object-storage.<后缀>`。通过`POST /api/platform/v1/skills/{skill_id}/calls`调用注册所得不可变UUID，input为对应页面的对象，业务输出位于`SkillCallResult.call.output`；所有Schema拒绝未知字段，无业务流式输出。

对象按核心可信subject隔离。Key是NFC UTF-8逻辑标识，最多1024字节，规则和共享metadata模型见[接口规范](../../preset-plugins/object-storage/interfaces.md)；没有业务owner参数。

单对象0..64MiB，文件沿核心Blob流式传输；put负责持久复制，export负责创建有TTL的临时Blob。源Blob、持久对象与导出Blob的生命周期独立。

put、export、delete使用稳定幂等键且持久保存结果，默认保留24h；list、stat、usage-get为纯读。大量操作使用[核心异步batch](../batches.md)，逐项遵守原契约并查询结果，不使用同步无限批量输入，不提供跨项事务。
