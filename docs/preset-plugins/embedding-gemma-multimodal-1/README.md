# embedding-gemma-multimodal-1 预设

[文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

插件ID：`org.xelab.embedding-gemma.multimodal-1`。使用 [统一模型锁](../embedding-gemma-model.md)，作为独立容器/安装版本/Skill，不与其他预设合并成一个动态线程开关。

| 项目 | 预设 |
|---|---|
| 模态 | 文本、图像、音频、视频组合 |
| CPU / 内存 | 1 vCPU / 6 GiB |
| 活动并发 / 副本 | 1 / 默认1 |
| 推理线程 / 模型份数 | 1 / 单进程单常驻模型 |
| 单项token预算 | 4096，含前缀与模态 |
| 单请求输入项 | 1，逐项有序输出 |
| 默认维度 | 768；可明确选128/256/512/768 |

[配置](configuration.md) · [接口](interfaces.md) · [部署与验收](deployment.md) · [清单](manifest.json) · [模板](deployment.template.json)
