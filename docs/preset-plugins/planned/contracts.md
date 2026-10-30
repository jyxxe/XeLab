# 规划能力逐接口索引

[规划总览](README.md) · [共享接口规则](interface-rules.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。共37个能力入口。

| 能力 ID | 插件目录 | 逐字段定义 |
|---|---|---|
| `ai.chat` | [ai-chat-openai-compatible](../ai-chat-openai-compatible/README.md) | [输入、输出与执行约束](../ai-chat-openai-compatible/contracts/chat/README.md) |
| `document.parse` | [document-parser](../document-parser/README.md) | [输入、输出与执行约束](../document-parser/contracts/parse/README.md) |
| `http.request` | [http-client](../http-client/README.md) | [输入、输出与执行约束](../http-client/contracts/request/README.md) |
| `notification.send` | [notification](../notification/README.md) | [输入、输出与执行约束](../notification/contracts/send/README.md) |
| `database.query` | [database-postgres](../database-postgres/README.md) | [输入、输出与执行约束](../database-postgres/contracts/query/README.md) |
| `database.execute` | [database-postgres](../database-postgres/README.md) | [输入、输出与执行约束](../database-postgres/contracts/execute/README.md) |
| `trigger.webhook` | [automation-webhook-trigger](../automation-webhook-trigger/README.md) | [输入、输出与执行约束](../automation-webhook-trigger/contracts/webhook/README.md) |
| `email.send` | [email-connector](../email-connector/README.md) | [输入、输出与执行约束](../email-connector/contracts/send/README.md) |
| `archive.extract` | [archive-processor](../archive-processor/README.md) | [输入、输出与执行约束](../archive-processor/contracts/extract/README.md) |
| `archive.create` | [archive-processor](../archive-processor/README.md) | [输入、输出与执行约束](../archive-processor/contracts/create/README.md) |
| `ai.rerank` | [ai-reranker](../ai-reranker/README.md) | [输入、输出与执行约束](../ai-reranker/contracts/rerank/README.md) |
| `browser.navigate` | [browser-automation](../browser-automation/README.md) | [输入、输出与执行约束](../browser-automation/contracts/navigate/README.md) |
| `browser.extract` | [browser-automation](../browser-automation/README.md) | [输入、输出与执行约束](../browser-automation/contracts/extract/README.md) |
| `git.repository` | [git-connector](../git-connector/README.md) | [输入、输出与执行约束](../git-connector/contracts/repository/README.md) |
| `feed.read` | [rss-reader](../rss-reader/README.md) | [输入、输出与执行约束](../rss-reader/contracts/read/README.md) |
| `json.validate` | [micro-json-validator](../micro-json-validator/README.md) | [输入、输出与执行约束](../micro-json-validator/contracts/validate/README.md) |
| `json.project` | [micro-json-project](../micro-json-project/README.md) | [输入、输出与执行约束](../micro-json-project/contracts/project/README.md) |
| `json.merge` | [micro-json-merge](../micro-json-merge/README.md) | [输入、输出与执行约束](../micro-json-merge/contracts/merge/README.md) |
| `convert.csv-json` | [micro-csv-json](../micro-csv-json/README.md) | [输入、输出与执行约束](../micro-csv-json/contracts/csv-json/README.md) |
| `convert.json-csv` | [micro-csv-json](../micro-csv-json/README.md) | [输入、输出与执行约束](../micro-csv-json/contracts/json-csv/README.md) |
| `convert.markdown-html` | [micro-markdown-html](../micro-markdown-html/README.md) | [输入、输出与执行约束](../micro-markdown-html/contracts/markdown-html/README.md) |
| `text.normalize` | [micro-text-normalizer](../micro-text-normalizer/README.md) | [输入、输出与执行约束](../micro-text-normalizer/contracts/normalize/README.md) |
| `text.regex-extract` | [micro-regex-extract](../micro-regex-extract/README.md) | [输入、输出与执行约束](../micro-regex-extract/contracts/regex-extract/README.md) |
| `datetime.convert` | [micro-datetime-convert](../micro-datetime-convert/README.md) | [输入、输出与执行约束](../micro-datetime-convert/contracts/convert/README.md) |
| `flow.condition` | [micro-flow-condition](../micro-flow-condition/README.md) | [输入、输出与执行约束](../micro-flow-condition/contracts/condition/README.md) |
| `flow.map` | [micro-flow-map](../micro-flow-map/README.md) | [输入、输出与执行约束](../micro-flow-map/contracts/map/README.md) |
| `flow.filter` | [micro-flow-filter](../micro-flow-filter/README.md) | [输入、输出与执行约束](../micro-flow-filter/contracts/filter/README.md) |
| `flow.template` | [micro-flow-template](../micro-flow-template/README.md) | [输入、输出与执行约束](../micro-flow-template/contracts/template/README.md) |
| `data.redact` | [micro-data-redactor](../micro-data-redactor/README.md) | [输入、输出与执行约束](../micro-data-redactor/contracts/redact/README.md) |
| `url.parse` | [micro-url-parser](../micro-url-parser/README.md) | [输入、输出与执行约束](../micro-url-parser/contracts/parse/README.md) |
| `file.metadata` | [micro-file-metadata](../micro-file-metadata/README.md) | [输入、输出与执行约束](../micro-file-metadata/contracts/metadata/README.md) |
| `crypto.hash` | [micro-hash](../micro-hash/README.md) | [输入、输出与执行约束](../micro-hash/contracts/hash/README.md) |
| `text.chunk` | [micro-text-chunk](../micro-text-chunk/README.md) | [输入、输出与执行约束](../micro-text-chunk/contracts/chunk/README.md) |
| `text.encoding-convert` | [micro-text-encoding](../micro-text-encoding/README.md) | [输入、输出与执行约束](../micro-text-encoding/contracts/encoding-convert/README.md) |
| `convert.xml-json` | [micro-xml-json](../micro-xml-json/README.md) | [输入、输出与执行约束](../micro-xml-json/contracts/xml-json/README.md) |
| `data.sort` | [micro-data-sort-group](../micro-data-sort-group/README.md) | [输入、输出与执行约束](../micro-data-sort-group/contracts/sort/README.md) |
| `data.group` | [micro-data-sort-group](../micro-data-sort-group/README.md) | [输入、输出与执行约束](../micro-data-sort-group/contracts/group/README.md) |

字段、分支条件与语义共同生效；原始Schema文件和四文件摘要均可从逐字段页面进入。
