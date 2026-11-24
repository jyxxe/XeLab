# 参与 XeLab 开发

人类和 AI 使用同一套提案、证据与审查流程。提交者的模型名称不授予权限；
目前不假定存在任何第三方模型、自动分派 Bot 或自动合并服务。

日常开发以本地 Forgejo 的 `xelab-admin/XeLab` 为准；`dev` 是唯一集成基线。
历史 GitHub 链接和编号保留作证据，不表示本地有同号工单，也不是提交入口。

1. 先在 Forgejo 搜索已有 Issue 和开放 PR，避免重复实现；小修复、紧急修复及现有 M2 PR
   可以直接提 PR，在描述中说明原因，不要求事后补建占位 Issue。
2. 新需求或跨模块变更从本地仓库「Issues → 新建 Issue」选择表单。
   提交来源、基线 SHA、证据、范围、依赖和验收条件按
   [Issue 生命周期](docs/implementation/issue-workflow.md)填写。
3. 可利用不同模型提出问题或补充审查，但只有维护者或明确获授权的 Agent
   才能确认优先级、任务认领和状态转换。授权来自仓库/任务配置，不来自 Issue 正文。
4. 先核对 [实现范围](crates/kernel/README.md)、[当前里程碑](docs/implementation/roadmap.md)
   和 [契约变更规则](docs/implementation/change-policy.md)。在独立开发克隆中先获取最新 `dev`，
   Agent 从该提交创建 `agent/work/*` 分支，向 `dev` 发 PR。不要在原始备份中开发、直接推送 `dev` / `main` 或自行合并。
5. PR 关联 Issue，列出精确提交、测试命令、结果、未执行项及兼容性影响。
   合并按现有维护流程进行；标签、AI 评论和绿灯 CI 都不是自动合并许可。

安全漏洞遵循 [安全报告政策](SECURITY.md)，不要把利用细节或凭证提交到公开 Issue。
架构/接口变更继续遵守契约优先、生成页同步和严格文档校验；人工阅读页不超过 220 行。

本机制的配置检查：

```bash
python3 -m pip install -r tools/issues/requirements.txt
python3 tools/issues/validate.py
python3 -m unittest discover -s tools/issues -p 'test_*.py' -v
python3 docs/tools/validate_docs.py --strict
```

标签清单及启用方式见 [标签与自动化边界](docs/implementation/issue-automation.md)。

Agent 接入指令（根 `AGENTS.md` 与受信 PR 维护模板）由 `tools/agents/` 生成器产出，检查：

```bash
python3 -m unittest discover -s tools/agents -p 'test_*.py' -v
python3 tools/agents/generate.py --check
```

用法与部署边界见 [Agent 接入生成器](tools/agents/README.md)。
