# skills 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/skills` | [列出可见的 Skill 版本](listSkills.md) |
| POST | `/api/platform/v1/skills` | [校验并注册不可变 Skill](registerSkill.md) |
| GET | `/api/platform/v1/skills/{skill_id}` | [读取 Skill 定义和可用性](getSkill.md) |
| DELETE | `/api/platform/v1/skills/{skill_id}` | [归档 Skill 版本](archiveSkill.md) |
| PUT | `/api/platform/v1/skills/{skill_id}/enabled` | [启用或禁用 Skill 路由](setSkillEnabled.md) |
| POST | `/api/platform/v1/skills/{skill_id}/calls` | [经核心转发调用插件业务接口](invokeSkill.md) |
| POST | `/api/platform/v1/skills/{skill_id}/calls:stream` | [经核心转发 Skill 流式响应](streamSkill.md) |
