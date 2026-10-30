# 分阶段落地与验收

[规划总览](README.md) · [平台里程碑](../../implementation/roadmap.md) · [落地门槛](feature-gates.md)

> 规格版本：2.8.0；状态：planned。顺序为依赖建议，不是已承诺的上线日期。

| 阶段 | 范围 | 前置条件 |
|---|---|---|
| P1：纯JSON数据处理 | json.validate/project/merge、flow.condition/map/filter、data.sort/group、url.parse、file.metadata的declared模式 | M1身份授权、MPR pure-json-v1、静态库与Schema绑定 |
| P2：文本与格式库 | CSV、Markdown、normalize、regex、datetime、template、redact、hash、chunk、encoding、XML | P1、固定库制品、Unicode/时区/tokenizer版本、资源与恶意输入验收 |
| P3：受控外部服务 | http.request、ai.chat、ai.rerank、email.send、notification.send、feed.read | M2密钥/Blob、M3 Skill、网络策略与别名ACL、计费/发送证据、可恢复幂等 |
| P4：文件与数据库 | document.parse、archive.extract/create、database.query/execute、git.repository | M2 Blob/卷限额、固定驱动/解析库、SQL/仓库范围授权、失败清理与持久幂等 |
| P5：可信入口及浏览器 | trigger.webhook、browser.navigate/extract | M4事件/任务适配、认证入口、浏览器隔离与全资源网络拦截、会话授权 |

每项实现先确定未实测的CPU/内存/并发/存储建议，经单节点CPU测量后发布部署模板。MPR 1.2.0的微部署默认250mCPU/256MiB，所有worker共享该部署的逐微cgroup硬上限，默认并发1/硬上限4，JS heap默认16MiB为子上限；核心适配器配置隔离，MPR不取得宿主cgroup权限，不创建每微OCI。

父MPR默认为4000mCPU/4294967296字节（4vCPU/4GiB），工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、缓存256MiB；默认最多4个不同微部署同时活跃，总活动槽4只为上限。所有阶段必须保留内核至少4C/16GiB及宿主基础设施预算，不用降低内核预留解决新增插件资源不足。

## 每个能力的发布检查

1. 确定完整输入/输出/流和错误语义，固定依赖及模式；满足所有适用gate。修正草案冲突后冻结正式契约版本。
2. 普通插件构建签名OCI镜像、正式Manifest、配置Schema及默认disabled部署模板；微插件构建实际源码、受限MicroManifest、独立包摘要与运行时兼容声明。
3. 打包Schema引用闭包，重新计算正式契约摘要；微Schema必须转换为运行时接受的MicroBusinessSchema，拒绝不支持的引用或关键字。
4. 按核心规则申请最小授权与固定依赖。测试Guest、子账户、权限组/API Key、父级撤权、别名ACL、跨用户Blob/会话访问及MPR使用权。
5. 验证正常/空输入、类型、边界、Unicode、数据对应关系、超限与取消。测试每种支持的模式和指定库版本，未提供的模式明确拒绝。
6. 对外部副作用注入“发送前失败、接受后断线、回复前崩溃、重复请求、配置或权限撤销”，检验未知状态及不重复发送。Blob输出检验配额、半成品清理与幂等账本。
7. 经过真实实现验收后才从planned迁入正式预设目录机器清单、注册候选Skill，再显式启用；目录存在、Schema通过和文档摘要不代替发布。

发布前另验收[微部署settings接口](interface-rules.md#微部署资源与联网设置)：默认network_enabled=false；Prepare/Ready与pure执行正常，controlled Invoke在worker创建前返回network_enabled对应的PERMISSION_DENIED且不发送或fallback。资源/联网手改必须按“停用 → disabled且排空 → 带ETag及幂等键更新 → 重新启用”，更新generation/revision并使旧句柄失效，不改变原limits、微版本、父绑定或Skill定义。

## 静态检查范围

[文档工具](../../tools/README.md)检查32个目录/37个能力完整性、原始契约摘要、共享引用、逐字段页面、正反结构样例及规划与现有安装清单隔离。它不连接模型/数据库/邮件/搜索服务，不执行浏览器、解析器、脚本或真实文件服务；运行时性能与隔离须在相应实现中验收。
