# 工作流定义与输入映射

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 发布与图校验

WorkflowSpec 包含 name、input_schema 和最多 64 个 WorkflowStep。step.id 在图内唯一，depends_on 只能引用已定义节点，拓扑排序必须覆盖全部节点；循环、未知依赖和重复 ID 返回 400。

每个版本不可变，提交修订创建新对象，逻辑版本递增；已有任务固定 workflow_id，不随“最新版本”漂移。归档阻止新建任务，不删除已有结果。

WorkflowStep以skill_id或contract二选一。skill_id锁不可变定义/安装及任务提供者绑定，不能同时覆盖provider_deployment_id。skill_required插件不接受裸contract节点绕过路由；详见 [Skill](../skills/routing.md)。

## input_mapping v1

input_mapping 为输出对象，其每个叶子只允许以下三种形式：

```json
{"literal":"固定值"}
```

```json
{"task_input":"/question"}
```

```json
{"step_output":{"step":"parse","pointer":"/text"}}
```

literal 可以是任意符合业务 Schema 的 JSON；task_input/step_output 使用 RFC6901 JSON Pointer。step_output.step 必须为已完成的直接或传递依赖。缺失路径为步骤 INVALID_ARGUMENT，不转为空字符串。禁止脚本、模板求值、环境变量和宿主文件读取。

## 执行与提供者

提交任务检查创建者对流程和所有被委托动作的当前权限，冻结委托上限。运行时每步仍重新核对，不能把发布者权限赋给执行者。状态相关步骤按任务固定提供者，绑定随任务保存。

maximum_attempts 包含首次，默认 1，最多 5；重试安全仍由能力 side_effect 决定，次数大于 1 不等于允许重复副作用。每步 timeout_ms 不得超过任务整体 deadline，单步最长 24h。

同步Skill节点由核心worker经Broker执行，并每10s续租；仅长期automation.execute节点使用TaskWorker及插件Heartbeat。同步Skill还受自身maximum_timeout_ms上限，知识库重建用多次有界批次而不是无限延长一次RPC。

接口见 [任务目录](../api/operations/tasks/README.md)，执行协议见 [RPC](../rpc/README.md)。
