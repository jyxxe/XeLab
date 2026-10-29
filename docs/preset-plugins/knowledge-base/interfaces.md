# 知识库接口索引与前置条件

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

所有业务接口使用 `POST /api/platform/v1/skills/{skill_id}/calls`，skill_id来自注册结果。表内是固定Skill版本的目标能力，不是另开未经认证的HTTP路径。

| Skill后缀 | 目标能力 | 逐接口定义 |
|---|---|---|
| `media-export` | `data.kb.media.export` | [导出媒体为临时核心Blob](../../skills/knowledge-base/media-export.md) |
| `media-import` | `data.kb.media.import` | [持久导入受控媒体](../../skills/knowledge-base/media-import.md) |
| `rows-delete` | `data.kb.rows.delete` | [删除明确行集合](../../skills/knowledge-base/rows-delete.md) |
| `rows-get` | `data.kb.rows.get` | [按ID读取行正文](../../skills/knowledge-base/rows-get.md) |
| `rows-query` | `data.kb.rows.query` | [完整ID列表链式检索](../../skills/knowledge-base/rows-query.md) |
| `rows-upsert` | `data.kb.rows.upsert` | [原子写入或更新行](../../skills/knowledge-base/rows-upsert.md) |
| `table-alter` | `data.kb.table.alter` | [修改知识表结构](../../skills/knowledge-base/table-alter.md) |
| `table-create` | `data.kb.table.create` | [创建知识表](../../skills/knowledge-base/table-create.md) |
| `table-drop` | `data.kb.table.drop` | [销毁知识表和所属数据](../../skills/knowledge-base/table-drop.md) |
| `table-get` | `data.kb.table.get` | [读取知识表结构](../../skills/knowledge-base/table-get.md) |
| `table-list` | `data.kb.table.list` | [列出本人知识表](../../skills/knowledge-base/table-list.md) |
| `vector-configure` | `data.kb.vector.configure` | [按字段启停向量索引](../../skills/knowledge-base/vector-configure.md) |
| `vector-drop` | `data.kb.vector.drop` | [明确删除字段向量数据](../../skills/knowledge-base/vector-drop.md) |
| `vector-reindex` | `data.kb.vector.reindex` | [有界批量更新待索引行](../../skills/knowledge-base/vector-reindex.md) |
| `vector-status` | `data.kb.vector.status` | [读取字段索引状态](../../skills/knowledge-base/vector-status.md) |

## 版本与副作用

- table-create分配表/字段UUID；table-alter按expected_schema_revision进行CAS，不能修改owner或静默重解释类型。
- rows-upsert/delete最多64行，同批全成或全败；更新必须匹配行版本，缺列按replace/patch语义处理。
- media-import先校验源Blob权限与内容，再持久复制；media-export创建有TTL的核心Blob，不返回宿主路径。
- vector-configure启用前按两用途校验固定embedding主备候选、权限、模态与对应维度探针，确认同空间后才提交配置。
- vector-reindex是有界派生索引更新，按行提交；它与原子行写入不同，允许一批中部分已完成，状态可查询。
- table-drop需confirm_table_id一致；vector-drop需字段已关闭并显式confirm_drop_vectors。

每个输入/输出的嵌套结构由对应contracts/<Skill后缀>下Schema定义；types.schema.json提供共享人工查阅入口。非只读接口持久化幂等/结果，query不执行输入SQL。

## 用户batch与WebUI

上表为单次有界执行原语；常规batch使用[异步核心入口](../../skills/batches.md)提交、查询和领取逐项结果。WebUI的知识库窗口/索引状态组件可按[UI贡献示例](../../contracts/examples/plugin-ui.json)开发，当前并无已实现HTML页面。

## 查询契约版本

rows-query目标契约2.0.0：expression和可选input_ids形成完整ID列表链，响应ids/count/vector_stages/cache，无limit/projection/order_by/cursor。rows-get目标1.0.0按1..200明确ID读取正文；需要维持同一行水位则两个接口均成对传expected_*。

表结构响应与vector-configure/status目标契约升2.0.0以公开index_revision。其余未改结构的原目标保持1.0.0，原14个Skill均升2.0.0固定新插件安装；每项精确name/version/digest见签名清单与Skill JSON。
