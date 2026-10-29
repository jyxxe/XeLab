# Skill batch：异步提交与查询

[Skill目录](README.md) · [逐接口](../api/operations/batches/README.md) · [执行与恢复](../tasks/batches.md)

> 规格版本：2.6.0；状态：待实现的规范。batch默认异步，不用长HTTP连接等待整批结果。

## 两种粒度

业务批次由核心保存、排队、调度和查询。一个item代表一次完整的有界Skill调用，其input必须匹配该Skill原输入Schema；不是把原input内的每一行自动变成item。提交同一固定Skill的1..1000项，完整请求最大4MiB。

插件内部的小批次是执行原语：embedding文本每次最多16条、知识库upsert/delete每次最多64行、reindex每次最多128行。这些原语继续返回完整单次结果；常规批量导入、批量embedding、批量检索和索引维护由异步入口承接。

单次交互式查询、预览、字段探针仍可使用`/skills/{skill_id}/calls`。同步入口不自动转成202；`/v1/embeddings`也保持同步兼容。需要batch时显式使用以下协议，不能靠timeout大小猜测执行方式。

## 提交与领取

| 操作 | 精确定义 |
|---|---|
| `POST /api/platform/v1/skills/{skill_id}/batches` | [createSkillBatch](../api/operations/batches/createSkillBatch.md)：SkillBatchRequest → 202 SkillBatch，带Location/Retry-After |
| `GET /api/platform/v1/skill-batches` | [listSkillBatches](../api/operations/batches/listSkillBatches.md)：按可见范围列出批次 |
| `GET /api/platform/v1/skill-batches/{batch_id}` | [getSkillBatch](../api/operations/batches/getSkillBatch.md)：状态、计数、期限 |
| `GET /api/platform/v1/skill-batches/{batch_id}/items` | [listSkillBatchItems](../api/operations/batches/listSkillBatchItems.md)：按输入顺序分页列出逐项状态 |
| `GET /api/platform/v1/skill-batches/{batch_id}/items/{item_id}/result` | [getSkillBatchItemResult](../api/operations/batches/getSkillBatchItemResult.md)：成功项的完整SkillCallResult |
| `POST /api/platform/v1/skill-batches/{batch_id}/cancel` | [cancelSkillBatch](../api/operations/batches/cancelSkillBatch.md)：携ETag请求取消 |
| `POST /api/platform/v1/skill-batches/{batch_id}/resume` | [resumeSkillBatch](../api/operations/batches/resumeSkillBatch.md)：同一owner显式重新授权暂停批次 |
| `POST /api/platform/v1/skill-batches/{batch_id}/items/{item_id}/resolve` | [resolveSkillBatchItem](../api/operations/batches/resolveSkillBatchItem.md)：按证据核实unknown，不自动重放 |

提交需Idempotency-Key。核心验证所有item及权限，原子保存批次、输入顺序、Skill UUID/digest、installation_id、执行委托、稳定逐项业务键和outbox后返回202；此时仅表示接受。Schema错误、重复item.id或超限拒绝整批，不先执行一半再报提交错误。

示例：[异步embedding提交](../contracts/examples/skill-batch.json)。其每个item.input仍是原embedding输入，输出在对应SkillCallResult.call.output。向量数量和维度不从批次状态对象推断。

## 轮询与结果

按Retry-After轮询；默认1s，指数退避至30s并加入抖动。HTTP200表示查询成功，要读取state判断执行结果。items按固定index分页，游标绑定当前用户、batch_id及位置，15min有效；进度变化不使位置游标失效，各页不是同一数据库长快照。

成功项可在整批未结束时领取，领取不删除。queued/running等返回409 RESULT_NOT_READY；failed/canceled返回409 RESULT_UNAVAILABLE并从item.error读取原因；unknown返回409 OUTCOME_UNKNOWN。结果上限16MiB/项，超限以PAYLOAD_TOO_LARGE失败，不截断为成功JSON。

批次终止时设置completed_at和results_expires_at=completed_at+7天，未终止项的既有成功结果不提前过期。过期结果返回410 RESULT_EXPIRED；脱敏批次/状态元数据保留30天后GC，之后404。输入/完整错误/结果对象在7天到期后清除；审计按独立保留策略。结果里的导出Blob仍受各自TTL约束，不因为批次保留7天自动延长24h Blob。

## 事务、并发与计量

failure_policy=continue默认继续独立项；stop在首个已确认失败后停止领取新项，剩余queued标记canceled，在途项完成或取消后再汇总。整批不是跨项目数据库事务，也不承诺按提交顺序完成；关联item必须由调用方选择max_parallel=1并自行处理版本依赖，或使用DAG工作流。

每个upsert item内部最多64行仍是原子事务；两个item之间可部分成功。reindex原语按行提交并可部分失败，其外层item成功表示返回了合法统计，不保证failed=0或全字段已就绪，调用者还应查询vector-status。

max_parallel默认1、最多16；再与账户并发、实例共享并发和可用资源取最小。text-8的8并发是执行容量，不表示batch永远占满8个槽。核心不为batch借用4C16G内核预留。

每项第一次实际执行作为一个根业务调用计入当日额度，内嵌embedding等沿该项root_call_id共享额度；安全重试不重复计数。提交/轮询不计业务调用次数，但受HTTP限流。不可将1000项当作一次调用以绕过额度。没有开始的项不占活动并发槽。
