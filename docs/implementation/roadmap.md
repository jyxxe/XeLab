# 实施里程碑与接口落地

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## M1：单节点内核与身份闭环

清单验证、签名信任、安装/部署/实例分离、最小 OCI 启动、CPU/内存父级限制、基本能力调用、管理 API/CLI、Guest/权限组/子账户/API Key、撤权与审计。至少两个独立实现复用同一契约接入且不修改核心。

## M2：存储与可恢复运行

卷硬配额、受控 Blob、secret 绑定、宿主外备份、控制循环对账、故障重启预算、有状态迁移。完成内核/容器/OOM/卷满故障注入。

## M3：网关适配与 WebUI

核心Skill注册/授权/固定安装绑定及转发、声明式 HTTP、可信 OAuth code/PKCE/同意页、隔离来源、默认界面选择、官方 UI、失效后的 CLI 恢复。UI 用公开管理 API，不增加后门数据库接口。

## M4：依赖与事件

完整依赖解析、固定绑定、HostService、持久化事件、订阅租约及死信；验证强依赖环、撤权与重复投递。

## M5：工作流与文本 AI

三种EmbeddingGemma 2预设、知识表/字段/表达式/默认128维索引和可选embedding依赖；任务/步骤/尝试、固定执行委托、租约/fence、结果未知核实、检查点、cron、模型别名与明确兼容子集。覆盖任务运行期间父级撤权和宿主重启。

## M6：开发生态

SDK、打包/契约测试工具、发布审核、兼容性验证、灰度策略及资源计量。基础隔离/签名与授权不延后到 M6；多节点与 GPU 不包含在上述交付。

每个里程碑只将实现并验证的 operationId 标记为可用；未实现操作应返回明确不支持响应，不能伪造空成功。本目录定义完整目标契约，并不代表 M1 必须同时实现全部协议。

## 新增插件规划

[32个规划插件](../preset-plugins/planned/README.md)与[37个能力入口](../preset-plugins/planned/contracts.md)按[独立交付阶段](../preset-plugins/planned/delivery.md)实施：纯JSON处理、文本格式库、受控外部服务、文件/数据库、可信Webhook及浏览器。它们不扩大已有核心HTTP/RPC计数；缺少宿主任务提交、可信文件桥或运行时库时先补正式契约和兼容协商。

## Extension Plugin落地

在固定Skill、MPR及来源授权闭环后实现[扩展插件](../extensions/README.md)：独立扩展对象/版本/部署、显式版本化扩展点、ordinary/micro/extension typed宿主、完整DAG/16层限制、根ExtensionHostService及候选Skill。先提供有界data贡献，再接入固定handler Skill；MPR1.3须显式声明只读extensionData与监督器转发适配。

每项发布前验证独立来源、原根/逐宿主/handler授权交集、资源不重复计量、宿主先Ready、拓扑Prepare及逆序Release。未实现宿主不得宣称支持扩展；缺扩展点的旧版本仍按原摘要运行，不把普通插件安装流程当作扩展制品登记。
