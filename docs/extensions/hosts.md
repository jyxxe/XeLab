# 宿主、扩展点与依赖图

[扩展目录](README.md) · [清单](manifest.md) · [生命周期](lifecycle.md)

> 规格版本：2.8.0；状态：待实现的绑定契约。每个扩展部署必须且只绑定一个宿主。

## 三类精确宿主引用

| `kind` | 必须固定的身份与制品 | 物理执行根 |
|---|---|---|
| ordinary | `deployment_id`、`installation_id`、`installation_digest` | 固定普通部署当前受审OCI实例 |
| micro | `micro_deployment_id`、`micro_version_id`、`package_digest` | 该微部署固定MPR安装/部署上的微资源组及worker |
| extension | `extension_deployment_id`、`extension_version_id`、`package_digest` | 沿上层宿主链递归解析到ordinary或micro根 |

类型分支互斥，不允许在一个引用中同时携带普通与微部署ID。所有正文ID只是待验证目标，核心必须读取真实对象、核对版本/摘要/所属关系和当前权限；不能相信调用者自报的owner、root或实例地址。

宿主类型指逻辑宿主，不混同实际提供RPC的进程。扩展宿主没有自己的网络实例；核心把完整逻辑链交给实际物理根的ExtensionHostService。微根也不接受用户直接连接MPR或替换runtime安装。

## ExtensionPoint

| 字段 | 精确含义 |
|---|---|
| `point_id` | 宿主版本内唯一的公开扩展点，不是代码路径 |
| `contract` | 点契约的精确名称、版本及摘要 |
| `input_schema` / `output_schema` | 该点公共业务输入和输出，决定扩展候选Skill的字段 |
| `contribution_schema` | 接受的严格JSON贡献结构 |
| `config_schema` | 扩展部署的固定配置结构，不允许明文凭证 |
| `mode` | data或skill；扩展不得自行切换 |
| `side_effect` | 该点已承诺的副作用分类；下层处理器不得超出 |
| `maximum_timeout_ms` | 该点最大期限，最多30000ms；仍与根调用deadline求交 |

普通宿主同时声明ExtensionHostService支持协议；只写扩展点而没有实际受审适配器不能Ready。MPR1.3只对明确声明相应点的微版本提供扩展数据/固定处理器监督路径，旧微版本继续原行为。

微宿主data点的业务输入、输出和副作用必须与其原MicroManifest业务契约一致，不能借一个扩展把纯文本输入改成原始文件访问，或把none操作变成外部计费。需要扩大契约时发布新微版本及相应Skill。

扩展还可声明自己的点，让下一层扩展提供独立贡献。下层点不能扩大任一祖先点的模式、权限、副作用或网络边界；物理根必须明确支持完整链及对应数据适配规则，否则拒绝准备。

## 完整宿主链

核心创建部署时解析typed host、所有扩展版本/包、点契约、固定config/limits及处理器引用，生成稳定host_chain_digest。它不包含健康、enabled、revision、实例generation或微部署可变resources/network设置，不能因重启而漂移公共Skill目标。

执行时另读取runtime_snapshot，包括实际root实例及代次、每层当前部署代次和状态。Prepare句柄绑定逻辑closure_digest及该运行快照；旧实例或旧部署代次不能使用新句柄。稳定链摘要与运行观测代次不能互相替代。

宿主绑定、扩展版本、config或limits在部署创建后固定；变化需新部署及新候选Skill。宿主重启只对同一固定逻辑链重新Prepare，不自动寻找同名最新宿主或另一个运行时。

## 无环与重入限制

扩展→宿主边是强依赖；skill模式的固定处理器依赖也纳入同一强依赖图，并与既有插件强依赖共同检查。拒绝自环、间接环和超过16层的扩展链；不能仅分别检查每一种插件类型后漏掉跨类型循环。

宿主发布扩展点不表示它必须等待所有扩展Ready，不建立反向强依赖。扩展可用性只影响对应点或候选，不使普通核心与其他未受影响业务依赖扩展启动。

运行调用还携带extension_path并禁止同一扩展重入；静态无环不能证明动态Skill回调不会递归。所有层共用根deadline、取消链及有界委托，不允许每层重新获得完整超时或复制调用额度。

单个Call冻结完整扩展集合、顺序、版本和定义摘要；排序不能依赖随机发现或当前容器地址。batch/工作流恢复使用已保存的固定扩展链，不按新安装扩展、同名最新版本或新处理器重放旧任务。
