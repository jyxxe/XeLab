# 业务接口与Skill绑定

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

唯一业务入口为 Skill `org.xelab.embedding-gemma.text-8.embed`，版本1.0.0；注册后使用返回的UUID调用 `POST /api/platform/v1/skills/{skill_id}/calls`。

目标为 CapabilityService.Invoke，契约 `ai.embedding.text@1.0.0`。business_entry=skill_required，旧 `/calls`、裸HostService.Invoke及无Skill模型别名均不能绕过。

完整输入/输出及错误语义见 [Skill文档](../../skills/embedding-gemma/embedding-gemma-text-8.md)。本预设不支持InvokeStream，流式入口返回INVALID_ARGUMENT。

文本预设可经绑定Skill的 `/v1/embeddings` 别名适配；多模态使用原生Skill输入。返回结果始终包含实际model_revision、profile_id、embedding_space_id和dimensions，不能只按模型显示名称复用索引。

Describe/Initialize/GetHealth/Drain/Shutdown、Cancel/QueryResult沿用平台RPC，属于控制/恢复接口，不作为用户技能发布。
