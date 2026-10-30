# 文档生成与验证

[文档总目录](../README.md) · [变更规则](../implementation/change-policy.md)

要求 Python 3.11+。基础检查只使用标准库；严格检查另需 requirements.txt。建议在项目专用虚拟环境安装，勿把验证依赖作为平台运行时依赖。

```bash
python3 -m venv .venv-docs
.venv-docs/bin/pip install -r docs/tools/requirements.txt
.venv-docs/bin/python docs/tools/validate_docs.py --strict
```

无法使用 venv 的环境也可安装到独立临时目录，再通过 PYTHONPATH 运行；不必修改全局 Python 包。

修改 OpenAPI 后生成 HTTP 操作与模型页：

```bash
python3 docs/tools/render_reference.py
```

修改 Protobuf 或 rpc-semantics.json 后生成逐 RPC 页：

```bash
.venv-docs/bin/python docs/tools/render_rpc.py
```

只检查目录、链接、引用和契约对应：

```bash
python3 docs/tools/validate_docs.py
```

严格模式额外校验 OpenAPI、所有 JSON Schema、请求与手工样例、TOML、Proto 编译及方法集合、SQL 语法。所有检查均只读，不连接部署、运行插件或应用数据库迁移。

api/、rpc/ 为生成区域；渲染器清理不再属于规范的生成页，不适合放手写说明。主题设计在 architecture/、identity/ 等目录维护。

内部方法生成：`python3 docs/tools/render_internal.py`。严格模式还核对7个OCI预设和1个微预设、用户指定embedding资源、24个静态Skill副本/目标、原始契约摘要、业务样例，以及知识库SQLite建库与默认128维。检索表达式解析器和模型尚未实现，此检查不代替真实执行验收。

2.2还校验异步batch响应、UI消息Schema及正反样例、主题条件字段、iframe窗口/组件互斥声明。上述校验只证明规格自洽，不表示已有浏览器实现。

2.3额外验证完整ID查询/行读取/缓存配置正反例，SQLite索引修订号触发器的insert、pending→ready、删除、generation变化及事务回滚。解析器、真实缓存LRU/TTL和查询执行尚未实现，这些检查不声称已经跑过链式查询。

2.4引入Micro Plugin Runtime与核心微对象、micro SkillTarget及MicroRuntimeService；严格校验源码/包摘要、动态候选Schema、目标混用/源码注入字段/引擎限制等边界。示例校验不表示已实现JS沙箱或实际运行脚本。

2.5新增网页搜索微制品/受控HTTP profile与对象存储6个Skill，校验服务商模板、密钥/URL字段隔离、微制品摘要、对象SQL修订水位/删除重建防ABA及用户主键隔离。未运行外部搜索API、真实文件服务或沙箱，结构检查不等于实现验收。

2.6新增32个planned目录和37个能力草案，检查独立编目、依赖/版本、原始文件与引用摘要、正反Schema样例及生成页面一致性。业务算法与外部服务尚未执行，不能据此声称插件可运行。新增规划的字段页面与索引通过下列命令生成：

```bash
python3 docs/tools/render_planned.py
```

修改草案契约后按四文件清单重算planned-catalog/planned-contracts中的摘要；引用的shared.schema.json/OpenAPI文件摘要同步写入reference_documents，不能以重生成页面掩盖原始契约漂移。

2.7增加微部署250mCPU/256MiB默认资源与默认关闭的联网设置、停用排空后手动修改接口，以及MPR父4vCPU/4GiB和逐微部署资源组契约。静态检查验证字段正反例、默认值、父工作池/控制预算算术、模板联网关闭及接口前置条件；不声称已经运行真实cgroup限额或联网撤销实验。

2.8新增Extension Plugin第一类资源、14个管理接口、三类typed宿主与四种Skill目标。validate_extensions.py校验3组扩展包/点契约/闭包的实际摘要、38个结构正反例及11个跨类型DAG/层深样例；它不启动加载器、宿主适配器或固定处理器。模型索引拆入api/models/index，每页最多100个条目，继续保持所有人工阅读页≤220行。
