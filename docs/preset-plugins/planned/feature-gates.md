# 落地门槛与依赖

[规划总览](README.md) · [实施顺序](delivery.md)

> 规格版本：2.6.0；状态：planned。gate是发布检查项，不是现有配置开关或自动授予的权限。

`contracts/`指[平台共享契约](../contracts/README.md)；`core-blob`指既有受控Blob协议；`platform-secrets`指平台密钥绑定；`network-policy`指平台网络准入。下表将清单中的门槛落实到必须补齐的组件及证据。

## 普通插件

| gate ID | 必须补齐或验证 |
|---|---|
| `external-ai-adapter` / `profile-configuration` | 模型别名、上游URL/密钥绑定、当前ACL、文本兼容子集映射、流背压与计费未知处理 |
| `document-parser-libraries` | PDF/DOCX/TXT/HTML版本锁定解析库；压缩/实体/页面/字符限额；禁宏、默认OCR和外链读取 |
| `http-policy-binding` | 目标域名/端口/方法及别名ACL配置；每次连接与重定向校验DNS/IP/SSRF；屏蔽敏感与逐跳头 |
| `response-blob-streaming` | 大响应到核心Blob的有界写入、授权、quota、摘要、取消清理；不得无限积累内存 |
| `notification-route-binding` | 受控渠道路由、收件人规则、速率及幂等记录；复用email能力时声明固定依赖和最小委托 |
| `delivery-evidence-recovery` | 上游接受/最终送达/未知的证据模型、幂等能力及故障恢复；后台队列、状态查询和回执API为后续独立设计 |
| `database-profile-acl` | 只允许批准的数据库连接、库/schema/table范围及凭证，最小数据库角色；只读事务与连接池限额 |
| `sql-policy-parser` | 驱动参数绑定及SQL结构策略；单语句、函数/DDL/管理操作限制；只做字符串前缀检查不合格 |
| `trusted-webhook-ingress` | 外部入口、签名/时间窗/重放校验、大小限额与可信认证记录；来源及原始请求绑定，不能信任body自报已认证 |
| `host-task-submission` | 任务模式需新增受授权的宿主提交适配及契约；现有HostService没有CreateTask。事件模式可复用PublishEvent，不须开放任务模式 |
| `email-transport-profile` | SMTP或服务商适配、Secret引用、发件人/收件人ACL、模板绑定、总收件人与附件字节限额 |
| `archive-libraries` | ZIP/TAR锁定库；路径穿越、链接/设备、条目深度、大小、压缩比与取消限额 |
| `bounded-multi-blob-output` | 解压多个结果Blob的总quota、原子发布与失败清理；幂等记录复用同一批Blob |
| `reranker-adapter` / `model-artifact-lock` | 远程计费证据或本地CPU模型；锁定模型/tokenizer来源版本摘要，输出候选ID/索引对应及稳定并列排序 |
| `browser-sandbox` | 固定浏览器引擎、禁下载/宿主文件/任意扩展、站点进程隔离、会话资源和生命周期 |
| `session-authorization` | 会话绑定当前主体与固定部署，超时回收；同用户子账户/其他部署不得凭UUID借用会话 |
| `all-resource-network-interception` | 导航、子资源、iframe、XHR/fetch、WebSocket、worker、重定向均执行网络策略；只检查首页URL不合格 |
| `git-transport-policy` | 批准Git传输和仓库地址，禁shell/helper/hooks/自动submodule/LFS，限制克隆与读取规模及超时 |
| `repository-profile-acl` | 私有仓库凭证绑定、分支/文件范围及主体ACL；固定resolved_commit，输出Blob的权限及幂等清理 |
| `feed-parser` / `http-mode-profile` | RSS/Atom安全XML解析；批准的http.request依赖或自有受控网络；外链不自动读取，源字节和文章数有界 |

## 微插件

所有微规划先通过 `pure-json-v1` 的源码/静态Schema/资源与错误协议验收。当前MPR不承诺安装Node/Python包、任意import、宿主文件、网络、shell或动态依赖。需要库时静态打包进受限JS制品或明确扩展并版本化运行时库；后者需先更新内核能力协商与契约，不能仅修改微插件清单。

| gate ID | 必须补齐或验证 |
|---|---|
| `restricted-schema-validator-library` | 当前MicroBusinessSchema白名单的有界校验器；拒绝外部引用、远程Schema、regex/format扩展及未知关键字 |
| `json-pointer-library` | RFC6901自有成员访问、严格数组索引、缺失语义、目标重叠/危险键拒绝 |
| `json-merge-library` | 浅/深合并、后值覆盖、replace/concat数组规则、全局深度及增长限额 |
| `csv-rfc4180-library` | 引号/换行/分隔符/重复表头、缺失列和显式null规则；无类型猜测与公式执行 |
| `markdown-parser-library` / `html-sanitizer-library` | 锁定Markdown方言及HTML清理策略；覆盖危险标签/事件/URL/外链，无宿主DOM权限 |
| `unicode-versioned-library` | 固定Unicode版本及规范化/空白定义，跨实现输出一致 |
| `linear-regex-library` | 固定线性受限引擎及语法子集；超输入/匹配/捕获组限额；不能默认宿主RegExp可抵御资源攻击 |
| `strict-rfc3339-library` | 严格时间解析、范围/偏移验证，无宿主TZ隐式回退；未知偏移拒绝 |
| `optional-tzdb` | IANA时区只在锁定tzdb版本后启用；缺少时保留UTC/固定偏移；歧义与不存在时间拒绝 |
| `condition-ast-library` | condition/filter共享AST≤256节点/深度16、类型敏感比较及缺失语义；不执行eval或知识库表达式 |
| `bounded-type-conversion` | 映射时只做严格JSON标量转换、溢出拒绝，不使用宿主truthy或隐式类型转换 |
| `template-parser-library` | 仅JSON Pointer占位及固定text/json/html过滤器；无任意代码、循环、include、文件或网络 |
| `redaction-library` / `optional-linear-regex` | 路径替换/码点遮罩规则、替换总数上限；regex-replace仅在线性引擎存在时开放 |
| `url-parser-library` | http/https URL标准化与查询顺序/重复参数/片段规则；不访问地址，不产出网络许可 |
| `declared-descriptor-library` | 纯JSON声明元数据与固定扩展名MIME表，输出trust=declared，不能声称已读取文件 |
| `verified-file-metadata-profile` | 将来经可信Blob桥取得实际文件属性时另行设计profile、权限及输入来源证明；当前declared模式不受此可选门槛阻塞 |
| `sha256-library` | 静态SHA-256实现、规范base64/UTF-8、标准测试向量与CPU/堆预算，不隐式开放宿主crypto模块 |
| `unicode-split-library` | 字符/段落规则、Unicode码点原文区间、覆盖、前进性及重叠预算 |
| `optional-tokenizer` | tokens精确模式的固定tokenizer及特殊token规则；每片最终单独编码验证，不以估算替代 |
| `token-estimator-library` | tokens-estimate模式固定utf8-div4-v1/0.1.0；输出显式estimated=true，下游Embedding仍重新检查真实token上限 |
| `charset-codec-library` | utf-8/utf-16le/utf-16be严格codec、BOM及替换计数；未知编码拒绝 |
| `secure-xml-library` | XML树/命名空间有界转换，拒绝DTD/XXE/PI/XInclude/网络实体，保留有序重复子节点与文本 |
| `typed-compare-library` | 标量类型敏感排序/分组、稳定并列顺序、null/缺失规则、组数与输出限额 |

这些门槛描述“满足什么才可发布”。可选模式仅在对应依赖缺失时禁用该模式；不得把缺失库的空成功、缺失时区的宿主回退或未验证文件的verified标记作为降级实现。
