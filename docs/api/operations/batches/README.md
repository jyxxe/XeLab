# batches 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| POST | `/api/platform/v1/skills/{skill_id}/batches` | [提交异步Skill批次](createSkillBatch.md) |
| GET | `/api/platform/v1/skill-batches` | [列出可见异步批次](listSkillBatches.md) |
| GET | `/api/platform/v1/skill-batches/{batch_id}` | [查询批次进度和状态](getSkillBatch.md) |
| GET | `/api/platform/v1/skill-batches/{batch_id}/items` | [分页查询逐项状态](listSkillBatchItems.md) |
| GET | `/api/platform/v1/skill-batches/{batch_id}/items/{item_id}/result` | [领取单项完整结果](getSkillBatchItemResult.md) |
| POST | `/api/platform/v1/skill-batches/{batch_id}/cancel` | [请求取消异步批次](cancelSkillBatch.md) |
| POST | `/api/platform/v1/skill-batches/{batch_id}/resume` | [显式重新授权暂停批次](resumeSkillBatch.md) |
| POST | `/api/platform/v1/skill-batches/{batch_id}/items/{item_id}/resolve` | [依据证据核实结果未知的项目](resolveSkillBatchItem.md) |
