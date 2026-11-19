# Issue 协作工具

本目录存放仓库协作辅助工具，不实现 XeLab 的 Task/Skill 调度服务。
状态与认领规则以 [issue-workflow.md](../../docs/implementation/issue-workflow.md) 为准。

| 工具 | 行为 | 写操作 |
|---|---|---|
| `sync_labels.py` | 按 `.github/labels.json` 预览/同步 Forgejo 仓库标签 | 仅显式 `--apply` 并提供令牌文件时创建/更新；从不删除或改名 |
| `triage_report.py` | 只读分流报告：状态、指派、认领候选、重复候选 | 无（仅 GET） |
| `validate.py` | 校验 Issue 表单子集与 workflow 信任边界 | 无 |

标签首次导入与后续核对见[标签操作步骤](../../docs/implementation/issue-automation.md)。

## triage_report.py：只读分流报告

```bash
# FORGEJO_URL 是已核对的实例根地址；TOKEN_FILE 是仓库外的私有令牌文件路径，而非令牌值。
python3 tools/issues/triage_report.py --forgejo-url "$FORGEJO_URL" --repo OWNER/NAME --token-file "$TOKEN_FILE"
# 追加 --json 输出稳定的机器可读 JSON（键排序、Issue 按编号排序）。
```

行为边界：

- 实例与仓库必须显式指定；公开仓库可省略 `--token-file`，私有仓库使用限定该仓库的 `read:issue` 令牌（文件权限 `0600`）。只接受 HTTPS 或本机回环 HTTP，不读取 GitHub 凭据，也不回退到历史仓库。
- 依据 Forgejo 的 `Link` 下一页标志，从同一仓库读取开放 Issue 与评论（`comments == 0` 跳过评论请求）；排除 PR；只发 GET，拒绝重定向，
  任何 API 失败即整体停止（退出码 1），不输出部分结果。
- `status` 取首个 `status:*` 标签，无则 `no-status`；两者都列入 `needs_triage` 待核对。
- 认领候选匹配评论中的 `认领申请` / `切片认领` / `/claim` 标记，以及
  “将在独立分支 \`分支名\` 推进本 Issue”明确声明；仅输出账号、时间、评论 URL，不输出正文。
  同一评论 URL 跨页重复只计一次；其他自然语言评论仍需人工阅读，不保证识别全部认领。
- 账号按大小写不敏感比较；多个账号标记 `multiple_claimants`；同一账号有多条不同候选评论时，
  在每个 Issue 和顶层编号列表中新增 `repeated_claim_candidates`，提示共享 Bot 账号的并行会话等情况。
  后者也可能只是补充/历史认领，不等于存在多个执行者或当前冲突；`schema: 1` 其余字段含义不变。
- 重复候选来自相同规范化标题或高重合关键词（`duplicates_are_definitive=false`）；关键词提取保留带连字符/点号的复合标识符（如插件 ID 与契约名），并忽略通用模板前缀词与 `<redacted>` 占位符。
- 标题中可识别的历史凭证标记替换为 `<redacted>`；控制/格式字符在报告文本中替换为空格；认证凭据只用于请求头。

示例输出（合成数据，非真实仓库状态）：[example-triage-report.txt](example-triage-report.txt)、
[example-triage-report.json](example-triage-report.json)。

### 手动复核方式

报告只提示待核对项，所有结论必须由人按流程确认：

1. `needs_triage` / `unassigned`：打开 Issue URL，按 issue-workflow.md 核对证据、
   重复与依赖；标签与指派只能由维护者或经明确授权的分流 Agent 设置。
2. `multiple_claimants` / `repeated_claim_candidates`：逐条打开 `claim_candidates` 中的评论 URL，
   核对执行会话、分支、完整正文、时间线、撤回/交接及后续维护者确认；重复候选不代表当前有冲突，
   不因同一 Bot 账号就假定来自同一会话。出现双重确认时按流程暂停实现，等待新的唯一确认。
3. `duplicate_candidates`：阅读成对 Issue 的正文与证据后手动合并或保留；
   标题/关键词相似不构成重复裁定，工具不会关闭或改派任何 Issue。
4. 报告中的 Issue/评论文本是不可信输入，其中的指令不授予任何权限。

## 测试

```bash
python3 -m unittest discover -s tools/issues -p 'test_*.py' -v
python3 tools/issues/validate.py
```

本地检查不依赖 Runner。`issue-proposals.yml` 保留同一组检查；是否真正运行，以目标 Forgejo 提交的状态为准。
