# tasks 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/workflows` | [列出工作流版本](listWorkflows.md) |
| POST | `/api/platform/v1/workflows` | [发布完整工作流版本](createWorkflow.md) |
| GET | `/api/platform/v1/workflows/{workflow_id}` | [读取工作流定义](getWorkflow.md) |
| DELETE | `/api/platform/v1/workflows/{workflow_id}` | [归档工作流版本](archiveWorkflow.md) |
| GET | `/api/platform/v1/tasks` | [列出本人或获授权任务](listTasks.md) |
| POST | `/api/platform/v1/tasks` | [提交持久化工作流任务](createTask.md) |
| GET | `/api/platform/v1/tasks/{task_id}` | [读取任务结果和状态](getTask.md) |
| POST | `/api/platform/v1/tasks/{task_id}/cancel` | [请求取消整个任务](cancelTask.md) |
| POST | `/api/platform/v1/tasks/{task_id}/retry` | [对可安全重试的失败步骤重新排队](retryTask.md) |
| GET | `/api/platform/v1/tasks/{task_id}/steps` | [读取任务步骤](listTaskSteps.md) |
| GET | `/api/platform/v1/task-steps/{step_id}/attempts` | [读取步骤执行尝试](listAttempts.md) |
| POST | `/api/platform/v1/task-steps/{step_id}/resolve` | [核实结果未知步骤](resolveUnknownStep.md) |
| GET | `/api/platform/v1/schedules` | [列出定时计划](listSchedules.md) |
| POST | `/api/platform/v1/schedules` | [创建带执行委托的定时计划](createSchedule.md) |
| GET | `/api/platform/v1/schedules/{schedule_id}` | [读取定时计划](getSchedule.md) |
| PUT | `/api/platform/v1/schedules/{schedule_id}` | [更新定时计划并重新核实授权](updateSchedule.md) |
| DELETE | `/api/platform/v1/schedules/{schedule_id}` | [停止后续计划触发](deleteSchedule.md) |
