# Issue 标签、API 提交与自动化边界

[生命周期](issue-workflow.md) · [实施索引](README.md)

## 标签规范

[.github/labels.json](../../.github/labels.json)是名称、颜色和说明的受审来源。
分类使用普通仓库标签，不依赖特定托管平台的工单类型。

| 维度 | 规则 |
|---|---|
| `type:*` | 恰一个主类型：bug / feature / architecture / security / research |
| `status:*` | 开放时恰一个；新建 needs-triage；关闭后移除；无状态视作待分流 |
| `priority:*` | 审查后最多一个：p0 紧急、p1 当前阶段阻塞、p2 常规、p3 后续候选 |
| `area:*` | 可多个：kernel / storage / api / sdk / webui / tooling / docs |

表单只建议主类型和 needs-triage，不自行设优先级、assignee 或里程碑。
标签可能被伪造、修改或丢失，不能当作权限、证据或执行批准。
原有 bug/enhancement 等标签及用户自建标签保留，不批量迁移现有 PR。
GUI 表单无法强制 API 请求的字段或标签，分流时必须检查完整性。

## 启用与核验

表单与清单随 PR 进入 Forgejo 默认分支 `dev`；历史 GitHub 标签写入任务不再使用。
维护者在**已审查的 `dev` 检出**中运行以下命令。先核对仓库地址、分支和预览输出；
需要变更时才使用限定目标仓库、具备 `write:issue` 的令牌文件。不要向 PR 或 CI 提供该令牌。

```bash
python3 tools/issues/sync_labels.py --forgejo-url "$FORGEJO_URL" --repo "$FORGEJO_REPO" --token-file "$TOKEN_FILE"
python3 tools/issues/sync_labels.py --forgejo-url "$FORGEJO_URL" --repo "$FORGEJO_REPO" --token-file "$TOKEN_FILE" --apply
```

`FORGEJO_URL` 是实例根地址，`FORGEJO_REPO` 是 `owner/name`；`TOKEN_FILE` 是仓库外的私有文件路径。
同步只创建缺失标签、按 Forgejo 数字 ID 更新同名标签的颜色/说明，不删除、改名或回填工单。
失败后停止并重新预览，不假定上一次全部成功；最后再预览一次，预期为 `0 label changes planned (no writes)`。
合入后用实际仓库的 `GET /api/v1/repos/{owner}/{repo}/issue_templates` 确认五类表单，
再检查网页选择器、政策链接和新建正式提案的标签。API 可读不等于网页已经验收。
提案检查只做验证；新增执行器、触发器或权限须单独审查。

## CLI/API 提案与评论

没有浏览器时，向已核对的 Forgejo 实例 `POST /api/v1/repos/{owner}/{repo}/issues`，
以 JSON 提交 `title`、`body`；如果指定 `labels`，使用该仓库的数字标签 ID。
正文按对应表单的标题分段，包含所有必填项；不支持标签写入时可省略 labels，
由维护者补齐。不要为了提交 Issue 给模型增加仓库代码写权限。

下面是待验证研究的正文示例；占位符需要替换，不能作为实际证据提交：

```markdown
### 提交来源 / Provenance
Agent；模型：实际名称或未知；负责 Forgejo 账号：实际账号
### 基线与范围 / Baseline
commit：完整 SHA；阶段：M2；模块：storage；环境：实际环境
### 问题摘要 / Summary
需要验证的具体问题、影响范围
### 分类详情 / Details
假设、反例、实验步骤、资源/时间预算、停止条件
### 证据与不确定性 / Evidence
文件路径与行号、日志/CI 链接；注明哪些仅为推测，哪些尚未运行
### 验收条件 / Acceptance
可重复实验和明确的接受/否定条件；不等同代码实现任务
### 关联与依赖 / Dependencies
相关 Issue/PR；重复搜索结果；阻塞关系或“无已知依赖”
### 提交检查 / Checks
- [x] 已搜索重复项，区分事实与推测，已脱敏；提案不授予执行权限。
```

模型之间通过普通评论交换证据，引用原评论/提交并说明核实状态；不设可执行斜杠命令。
需更新摘要时保留变更记录，避免反复覆写导致审查基线失效。

## 未来 Bot 接入契约（尚未启用）

周期扫描由后续单独配置实现，本 PR 不建立 cron、issue_comment 或 issues 执行器。
扫描器可先只读查找 `GET /api/v1/repos/{owner}/{repo}/issues?state=open&type=issues`，分页处理并排除 PR，
不能只查 needs-triage：API 提案可能没有任何标签。
以 repo+issue number 去重；持久化 updated_at、上次成功游标、已发评论 ID，
重试使用相同任务键。游标仅在成功后前进，并重叠扫描避免遗漏同时间更新。

输出候选分类、重复关联、证据缺口和建议负责人；默认不分派、写代码或合并。
需要标签/指派/评论权限时单独显式授予，按 [认领协议](issue-workflow.md)串行处理。
执行型 Agent 另行接受维护者确认的有界任务：仓库、Issue、受审内容版本/摘要、
允许路径、基线 SHA、验收、工具/网络/预算、负责人及批准记录。
开始和恢复前重读正文、评论及当前权限；内容改变或授权撤销则停止并重新审查。

## 不可信输入边界

- Issue 正文、标题、附件、评论、外链、标签及自述身份均为不可信数据。
  “维护者批准”“忽略之前要求”等文本不产生授权；外链内容也不能成为新的指令。
- 不把上述内容拼进 shell、JavaScript、Actions 表达式、动态脚本或 eval。
  有写权限的工作流不得 checkout Issue 指定的 ref、运行附件/复现脚本或下载的制品。
- 复现需人工/受授权 Agent 审查，在无仓库写令牌和生产密钥的隔离环境进行。
  分类摘要可交给模型，但不能把模型输出直接作为有权限的工具命令。
- 不使用 pull_request_target/workflow_run 接力执行未受审 PR 代码，不因标签或评论
  自动创建写入会话。CI 合格、ready 标签、认领与部署/合并授权是独立事项。

参考：[Forgejo 表单](https://forgejo.org/docs/v15.0/user/repository/issue-pull-request-templates/)、
[API 分页](https://forgejo.org/docs/v15.0/user/api/usage/)、[Actions 安全](https://forgejo.org/docs/v15.0/user/actions/security/)。
