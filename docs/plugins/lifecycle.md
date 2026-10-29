# 插件安装、启停与健康

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 安装与部署分开

installPlugin 验证并持久化不可变安装记录，不创建业务路由。createDeployment 保存期望配置、审批引用、资源预留与操作记录，控制器再创建容器。

启动顺序：准入 → 挂卷和受控网络 → 容器启动 → Describe 比对清单摘要 → Initialize 当前 generation → startup/ready 检查 → 逐能力注册 → 网关发布路由。

## 健康契约

- startup 默认最多 60s；单探针默认 2s，每 5s 一次，连续 3 次失败撤销可用性。
- alive 仅描述进程存活；能力 ready 单独描述该能力是否可调用。
- HealthResponse.observed_generation 必须等于期望代次，否则不可路由。
- 缺失强依赖时默认使依赖该服务的能力不可用；无关能力可继续服务。
- 健康恢复先验证当前审批、授权和依赖，不能直接重放旧路由快照。

## 禁用与停止

enabled=false 先撤销新路由，再 Drain，处理长期任务，停止容器并观测退出。排空期限默认 30s，超时尝试取消，最后由运行时终止。停止完成后释放 CPU/内存预留，卷和备份保留。

进程崩溃每代次最多重启 5 次，退避 1/2/4/8/16s 加 0–20% 抖动。稳定 Ready 10min 后可重置连续故障计数，仍保留总历史。资源不足保持 waiting_resources，不消耗重启次数。

uninstallPlugin 要求没有部署引用该安装。deleteDeployment 只归档部署；deleteVolume 才是显式业务数据销毁，不能联动默认删除。

RPC 定义见 [RPC 目录](../rpc/README.md)；状态见 [状态机](../architecture/state-machines.md)。
