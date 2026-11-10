# PR 验收证据报告（只读）

`pr_status.py` 按仓库与 PR 号汇总验收证据：精确 base/head SHA、Draft/冲突状态、
绑定当前 head 的 check runs 与 commit statuses、可读取时的必需检查、Review
（区分当前 head 批准与旧提交批准）以及未解决线程。供多 Agent 协作时人工核对，
不代替独立审查与合并门禁。

## 边界

- 全程只读：REST 仅 GET；线程计数使用 GraphQL 只读 `query`（工具拒绝 mutation）。
  不调用 approve/merge/rerun/评论，不更改仓库任何状态。
- 仅读取本机 `GITHUB_TOKEN`（公开仓库可匿名）；不输出、记录或提交 Token，
  报告不含 PR/Review 正文、标题等不可信文本，重定向时拒绝转发凭证。
- 读不到分支保护/必需检查配置时标记 `unknown`，绝不猜测；GraphQL 线程读取失败同样标 `unknown`。
- `merge_authorization` 恒为 `not-assessed`：检查完成不等于获准合并，合并由维护者按真实门禁决定。
- 退出码只表示报告是否生成：0 = 已生成（无论检查通过与否），1 = API/输入错误。

## 用法

```bash
python3 tools/review/pr_status.py --repo jyxxe/XeLab --pr 59        # 人类可读
python3 tools/review/pr_status.py --repo jyxxe/XeLab --pr 59 --json # 稳定 JSON（排序键，无本地时间戳）
```

checks 状态：`passed` / `pending` / `failed` / `action-required` / `none-reported`
（无任何检查时如实报告，不误报通过）。必需检查逐项为 `passed/pending/failed/action-required/missing`。

## 样例与测试

`example-report.json` 为离线夹具生成的**合成样例**（example/project，非真实仓库数据），
展示完整 JSON 结构。人工复核方式：对照报告中的 head SHA 打开 PR 页面与检查链接逐项确认。

```bash
python3 -m unittest discover -s tools/review -p 'test_*.py' -v
```

流程边界见 [Issue 工作流](../../docs/implementation/issue-workflow.md) 与
[标签与自动化边界](../../docs/implementation/issue-automation.md)；贡献规则见 [贡献指南](../../CONTRIBUTING.md)。