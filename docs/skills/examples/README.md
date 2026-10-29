# Skill调用样例

[Skill目录](../README.md)

- [文本embedding，显式128维](embedding-text.json)
- [知识表创建](kb-create-table.json)
- [按字段启用默认128维索引](kb-enable-vector.json)
- [len与topk组合查询](kb-query.json)

JSON是SkillInvokeRequest请求体；路径skill_id使用注册结果。样例UUID需替换，既不表明真实部署存在，也不代表用户已获权限。

- [多次topk链](kb-query-chain.json)
- [完整ID响应](kb-query-result.json)
- [把上次ID列表作为输入](kb-query-reuse.json)
- [按明确ID读取正文](kb-rows-get.json)

链式查询示例采用rows-query@2.0.0，返回ids/count；响应样例是独立业务输出对象，非SkillInvokeRequest。rows-get@1.0.0读取正文，示例的expected_*需来自实际查询响应。

- [默认查询/索引优先级启用字段](kb-enable-vector-default.json)
- [显式固定主备Skill](kb-embedding-routes.json)

- [对象存储空文件导入](object-put.json)
- [对象导出](object-export.json)
- [网页搜索微调用](../../preset-plugins/web-search/invoke.example.json)
