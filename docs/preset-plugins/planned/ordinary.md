# 普通插件规划目录

[规划总览](README.md) · [落地门槛](feature-gates.md) · [Skill规划](../../skills/planned/README.md)

> 规格版本：2.6.0；状态：planned；各业务契约草案为0.1.0。目录名与能力 ID 按需求保留。

普通插件运行在独立 CPU OCI 容器中，适合持续服务、外部连接、复杂依赖与独立资源配额。资源默认值须在实现测量后确定并写入正式部署模板。

| 目录 | 能力 ID | 依赖项 | 用途 |
|---|---|---|---|
| [ai-chat-openai-compatible/](../ai-chat-openai-compatible/README.md) | `ai.chat` | [共享契约](../contracts/README.md)；`platform-secrets`；`external-model-api` | OpenAI兼容文本生成，普通与流式 |
| [document-parser/](../document-parser/README.md) | `document.parse` | [共享契约](../contracts/README.md)；`core-blob`；`format-parser-libraries` | 解析PDF/DOCX/TXT/HTML为文本、页段及元数据 |
| [http-client/](../http-client/README.md) | `http.request` | [共享契约](../contracts/README.md)；`network-policy` | 受控通用HTTP请求 |
| [notification/](../notification/README.md) | `notification.send` | [共享契约](../contracts/README.md)；`platform-secrets`；`email.send-or-webhook-route` | 统一通知渠道编排、速率与幂等 |
| [database-postgres/](../database-postgres/README.md) | `database.query`、`database.execute` | [共享契约](../contracts/README.md)；`postgres-driver`；`platform-secrets`；`target-database` | 授权范围内数据库读写 |
| [automation-webhook-trigger/](../automation-webhook-trigger/README.md) | `trigger.webhook` | [共享契约](../contracts/README.md)；`api-gateway`；`platform-events-or-tasks` | 认证Webhook转事件或任务 |
| [email-connector/](../email-connector/README.md) | `email.send` | [共享契约](../contracts/README.md)；`platform-secrets`；`smtp-or-email-api` | 正文、模板与受限附件邮件发送 |
| [archive-processor/](../archive-processor/README.md) | `archive.extract`、`archive.create` | [共享契约](../contracts/README.md)；`temporary-storage`；`archive-libraries` | 受限归档创建/解压 |
| [ai-reranker/](../ai-reranker/README.md) | `ai.rerank` | [共享契约](../contracts/README.md)；`candidate-documents`；`remote-api-or-cpu-model` | 对候选文档或搜索结果重排 |
| [browser-automation/](../browser-automation/README.md) | `browser.navigate`、`browser.extract` | [共享契约](../contracts/README.md)；`browser-runtime`；`network-policy`；`optional-credential-profile` | 浏览器导航、页面读取和结构化提取 |
| [git-connector/](../git-connector/README.md) | `git.repository` | [共享契约](../contracts/README.md)；`git-library-or-client`；`repository-address`；`optional-secret-profile` | 只读仓库文件、分支和提交查询 |
| [rss-reader/](../rss-reader/README.md) | `feed.read` | [共享契约](../contracts/README.md)；`http.request-or-controlled-egress`；`rss-atom-library` | 读取RSS/Atom文章列表 |

这些目录不属于当前可安装预设，也没有自动注册的 Skill；[机器目录](../planned-catalog.json)与[契约目录](../planned-contracts.json)分别记录依赖、门槛及字段定义。

普通插件资源与 MPR 总资源都从插件池扣除；内核至少4 vCPU / 16 GiB预留、OS/基础设施预算及单节点不超卖规则不变。多项调用使用核心异步 batch，不在脚本内无限循环代替调度。
