# 配置与有界输入

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

[defaults.json](defaults.json) 是固定profile配置，Manifest.config_schema逐项锁定。改变并发、模态或预处理策略需要新profile并重新压测；提高获批CPU/内存不自动改变并发。

文本单项最多4096 tokens；每请求总计最多8192 tokens，超限拒绝，不静默截断。最长字符串还受输入Schema字节/长度限制。batch是单请求内输入数组，不等于多请求并发。

实例 max_active_requests=1，共享等待队列32，排队期限10s且不超过根deadline。文本8并发必须允许8个活动处理上下文/合批输入，不得只把8个串行待办伪称8并发；1vCPU是其总计算预算，并非8核或吞吐保证。

inference_threads=2，inter_op_threads=1，所有预处理和库线程均计入容器。禁止按并发数复制8份模型进程。生命周期/健康检查不等待业务槽。

load_vision=false、load_audio=false；任何图像/音频/视频请求在进入推理队列前拒绝，不自动转发多模态插件。

模型原始权重在只读镜像内，冷启动只读本地锁定文件。无运行时公网下载，Ready前完成摘要、模态支持和小样本有限数值检查。
