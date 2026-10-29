# Skill 定义、注册和不可变版本

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

完整字段：[SkillDefinition](../api/models/SkillDefinition.md)、[SkillTarget](../api/models/SkillTarget.md)。

## 制品和注册

普通插件清单skills[]给出候选定义，名称必须在plugin_id命名空间内。输入、输出、流式Schema与目标精确对应；普通定义由镜像和签名覆盖。发布者不能通过Skill改写核心管理路径或目标权限。

安装后候选定义不会自动启用。registerSkill 明确给出installation_id，由有权管理目标的管理员注册固定name/version，definition_digest 为RFC8785/JCS后SHA-256。相同name/version重复注册必须同时匹配定义及安装摘要，否则CONTRACT_CONFLICT。

路由固定声明来源的精确安装制品，不仅匹配plugin_id。插件/模型升级需发布对应新Skill版本，并显式迁移模型别名或知识库字段；旧任务保留旧UUID。不能让同接口新版模型静默接管旧索引的Skill。

Skill UUID 标识一个不可变版本。修改目标、Schema、资源标记、时限或映射须发布新版本，不允许原地更新。enabled/revision 属于运行管理状态，与定义版本分开。

## 可用性

enabled=false → disabled；目标缺失、未批准、依赖/健康失败 → unavailable；全部准入条件满足才 ready。可用性不是授权，调用前仍核对当前版本、目标和来源链。

归档撤销新路由并保留历史定义。运行任务和 Call 保存 UUID/digest，不能按同名“最新”漂移；恢复时不可用则按失败/等待策略处理。

## Schema 与 Blob 标记

input_schema/output_schema 为有效 JSON Schema 2020-12，不允许联网 $ref。input_blobs=true 时，仅识别受审输入 Schema 中 `x-xelab-resource-kind: blob` 标记的 UUID 字段；按实际匹配的 oneOf/数组分支提取，不递归信任任意业务 JSON 中名叫 blob_id 的字段。

核心逐个检查 Blob 读取权、MIME/容量基础约束，生成本次精确资源白名单。插件新建的输出 Blob 由 HostService 记录来源后才可用于后续获准子调用；资源标记不能扩大用户已有权限。

## micro目标与独立来源

SkillTarget.kind=micro时，plugin_id固定为org.xelab.micro-plugin-runtime，并固定micro_version_id、package_digest、micro_deployment_id。mapping仍为identity：业务input直接对应MicroManifest.input_schema，output对应其output_schema；不把MicroRuntimeService的控制字段暴露为用户业务封装。

微版本发布由内核校验当前授权owner、包/源码摘要和MicroManifest；创建MicroDeployment后才生成deployment.candidate_skill，固定确切micro_deployment_id及limits超时，不自动注册或启用。registerSkill核对生成候选与微版本/微部署一致，且CreateSkill.installation_id=version.runtime_installation_id=微部署所绑定运行时部署的实际installation_id。

微制品和MPR镜像是两种来源：微源码的发布记录及摘要不受原运行时OCI镜像签名自动覆盖。注册时不能仅因目标MPR安装存在就接受任意脚本定义，也不能让调用方替换candidate_skill内的目标、Schema、名称或版本。

微源码、包、Schema或运行时安装变化须发布新微版本；微部署limits或父运行时绑定变化须新建微部署。候选名为org.xelab.micro-plugin-runtime.micro.<micro_deployment_uuid>.run，版本为MicroManifest.version；不能原地改写已注册Skill或把旧batch迁到同名新版。微部署可在固定版本和limits下启停，准备句柄重建属于运行状态变化，不修改定义。

首版受限JS的stream_item_schema=null、input_blobs=false、side_effect=none；业务JSON中即使出现blob_id也不授予脚本读取资源的能力。完整关系见[微插件架构](../architecture/micro-plugins.md)。
