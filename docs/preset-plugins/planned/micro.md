# 微插件规划目录

[规划总览](README.md) · [落地门槛](feature-gates.md) · [Skill规划](../../skills/planned/README.md)

> 规格版本：2.8.0；状态：planned；各业务契约草案为0.1.0。目录名与能力 ID 按需求保留。

微插件统一依赖 `contracts/` 与 Micro Plugin Runtime 执行协议；由 MPR 加载，不为每个微插件创建容器。每微部署默认250mCPU/256MiB，总限额由其全部worker共享，联网默认关闭；资源与联网可在停用排空后通过settings接口手动配置。库须静态打包或由版本化运行时库明确提供；未声明的宿主模块不可调用。

| 目录 | 能力 ID | 依赖项 | 用途 |
|---|---|---|---|
| [micro-json-validator/](../micro-json-validator/README.md) | `json.validate` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`restricted-json-schema-validator` | 受限Schema/规则校验 |
| [micro-json-project/](../micro-json-project/README.md) | `json.project` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`json-support` | 按JSON Pointer提取重命名投影 |
| [micro-json-merge/](../micro-json-merge/README.md) | `json.merge` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`json-support` | 浅深对象合并与数组冲突规则 |
| [micro-csv-json/](../micro-csv-json/README.md) | `convert.csv-json`、`convert.json-csv` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`csv-parser-serializer` | CSV与JSON转换 |
| [micro-markdown-html/](../micro-markdown-html/README.md) | `convert.markdown-html` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`markdown-parser`；`html-sanitizer` | Markdown转清理后HTML |
| [micro-text-normalizer/](../micro-text-normalizer/README.md) | `text.normalize` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`unicode-normalization-library` | 空白换行Unicode规范化 |
| [micro-regex-extract/](../micro-regex-extract/README.md) | `text.regex-extract` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`restricted-regex-engine` | 受限正则提取 |
| [micro-datetime-convert/](../micro-datetime-convert/README.md) | `datetime.convert` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`datetime-library`；`optional-versioned-tzdb` | 日期时间与时区转换 |
| [micro-flow-condition/](../micro-flow-condition/README.md) | `flow.condition` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`declarative-condition-parser` | 声明式条件判断 |
| [micro-flow-map/](../micro-flow-map/README.md) | `flow.map` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`json-support` | 字段映射常量默认与简单类型转换 |
| [micro-flow-filter/](../micro-flow-filter/README.md) | `flow.filter` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`declarative-condition-parser` | 声明条件过滤记录集合 |
| [micro-flow-template/](../micro-flow-template/README.md) | `flow.template` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`restricted-template-engine` | 受限模板生成文本或结构化值 |
| [micro-data-redactor/](../micro-data-redactor/README.md) | `data.redact` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`redaction-rules`；`optional-restricted-regex` | 字段或文本模式脱敏 |
| [micro-url-parser/](../micro-url-parser/README.md) | `url.parse` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`url-parser` | URL结构解析，不访问网络 |
| [micro-file-metadata/](../micro-file-metadata/README.md) | `file.metadata` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`controlled-file-input`；`optional-mime-database` | 文件名扩展名大小MIME元数据 |
| [micro-hash/](../micro-hash/README.md) | `crypto.hash` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`sha256-library` | SHA256内容摘要 |
| [micro-text-chunk/](../micro-text-chunk/README.md) | `text.chunk` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`text-split-rules`；`optional-fixed-tokenizer` | 按字符段落精确token或显式估算切分 |
| [micro-text-encoding/](../micro-text-encoding/README.md) | `text.encoding-convert` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`charset-codec-library` | 字符编码转换与解码错误策略 |
| [micro-xml-json/](../micro-xml-json/README.md) | `convert.xml-json` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`secure-xml-parser`；`json-support` | XML与有序显式JSON树互转 |
| [micro-data-sort-group/](../micro-data-sort-group/README.md) | `data.sort`、`data.group` | [共享契约](../contracts/README.md)；[MPR](../micro-plugin-runtime/README.md)；`structured-json-processing` | 结构化记录稳定排序分组 |

这些目录不属于当前可安装预设，也没有自动注册的 Skill；[机器目录](../planned-catalog.json)与[契约目录](../planned-contracts.json)分别记录依赖、门槛及字段定义。

普通插件资源与 MPR 总资源都从插件池扣除；内核至少4 vCPU / 16 GiB预留、OS/基础设施预算及单节点不超卖规则不变。多项调用使用核心异步 batch，不在脚本内无限循环代替调度。
