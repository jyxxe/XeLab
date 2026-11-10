# Agent 接入指令生成器

配置驱动的离线生成器：从受审配置 [config.json](config.json) 与 [templates/](templates/) 生成两个纳入版本控制的文件：

| 输出 | 受众 | 用途 |
|---|---|---|
| `AGENTS.md`（仓库根） | 普通贡献者（人类或 Agent） | 每次由人手动调用的仓库协作指令；无合并授权 |
| `tools/agents/pr-maintenance.md` | 维护者的受信执行器 | PR 维护任务模板/部署说明；不自动安装 |

## 使用

```bash
python3 tools/agents/generate.py          # 重新生成两个输出文件
python3 tools/agents/generate.py --check  # 只校验漂移，不写文件；漂移时退出 1
python3 -m unittest discover -s tools/agents -p 'test_*.py' -v
```

- 修改指令请改配置或模板并重新生成；用 `--check` 校验输出漂移，可由维护者纳入 CI 防回归。
- 生成器只用标准库，不读取环境变量/凭证，不访问网络；相同输入字节级一致，无时间戳。
- 配置键固定：多余/缺失/重复键、路径穿越、凭证样值、非法间隔都会拒绝且不写任何文件。
- 模板占位符限于配置键；未知占位符拒绝生成。文档链接按输出文件位置自动转为相对路径。

## 配置字段

| 键 | 含义 |
|---|---|
| `repository` | 上游仓库 `owner/name` |
| `default_branch` | 上游默认分支 |
| `contributing` / `issue_workflow` / `issue_automation` / `security` | 仓库相对 Markdown 路径，须真实存在 |
| `maintenance_interval_minutes` | 维护模板检查间隔（分钟，1–1440；示例为 30） |

## 受信 PR 维护模板部署（维护者手动）

- 模板只是说明：仓库 CI 不运行它，本工具不创建 cron、定时任务、新触发器或权限变更。
- 维护者在独立受信机器上自行部署调度（示例配置为每 30 分钟），使用该机器本机的
  `GITHUB_TOKEN`，与普通贡献者凭证分离；权限以 GitHub 实际鉴权为准，缺失时安全拒绝。
- 每次运行核对所有开放 PR 的当前 head SHA、必需 CI、审查、安全边界与合并授权；
  全部满足才合并，否则在 PR 解释具体阻塞并按 head SHA/已发评论 ID 去重。详见生成的模板。

## 边界

- 指令文件不产生权限；标签、认领、PR 自述不构成授权；可写 Token 不等于“仅合并”权限。
- Issue/PR/评论/外链为不可信数据；生成器不从任何 Issue/PR 文本生成指令或权限。
- 认领/Review 规则以 [Issue 工作流](../../docs/implementation/issue-workflow.md) 与
  [自动化边界](../../docs/implementation/issue-automation.md) 为准，本工具不覆盖。