# 扩展插件验收条件

[扩展目录](README.md) · [生命周期](lifecycle.md) · [平台验收](../implementation/acceptance.md)

> 规格版本：2.8.0；状态：待实现的验收规范。静态文档校验不代替真实宿主、MPR或处理器测试。

## 制品与绑定

| 场景 | 必须验证的结果 |
|---|---|
| 三类宿主 | ordinary、micro、extension均能按声明点创建精确绑定；逻辑宿主不被当作网络实例 |
| 不支持扩展的旧宿主 | 缺扩展点/协议时拒绝准备；不写回旧Manifest/MicroManifest或重算原摘要 |
| 宿主引用混用 | oneOf分支严格互斥，普通ID配微字段、版本/包不匹配及伪造root拒绝 |
| 严格贡献JSON | BOM、重复key、非有限数、超过256KiB、多JSON或不符合contribution_schema均拒绝 |
| 模式 | data拒绝handler字段，skill要求固定UUID及definition_digest；输入不能改模式 |
| 来源 | 扩展包与贡献摘要按实际字节核验，宿主签名不替代扩展owner/来源登记 |
| 固定配置 | 创建后不能热换host、version、config、limits；新建部署不覆盖旧候选或旧Call |

## 依赖图与生命周期

验证扩展→普通、扩展→微、扩展→扩展以及混合链，合并宿主与处理器强依赖检测直接环、间接环和超过16层。宿主声明扩展点不人为建立反向强依赖；动态处理器回调重入还必须被extension_path拒绝。

覆盖默认disabled、不自动注册/启用候选、宿主先Ready、Prepare成功才路由、宿主失效waiting_host、同链恢复重新Prepare，以及停用/归档的逆序draining。核实旧runtime_snapshot、旧句柄和迟到结果不能跨代使用。

模拟宿主重启、MPR重启、worker退出、准备缓存淘汰、处理器停用及权限撤销。管理API继续可用；恢复不能自动改用同名最新安装、另一个MPR或新处理器。清理失败不能提前报告disabled/archived或删除Call证据。

## 授权与能力边界

使用Owner、Operator、Member、Auditor、Guest、子账户及派生API Key检查独立扩展资源授权；验证知道扩展/宿主/处理器UUID不构成使用许可。旧冻结权限上限不自动获得新增扩展动作。

分别撤销扩展执行权、宿主使用权、祖先权限、handler权限、Secret/HttpBinding和网络开关。缓存、已准备句柄、当前挂载关系或同用户关系均不能跳过当前检查；关闭网络不作为provider故障fallback。

检查data不执行JS/.so/shell、不访问宿主路径、不解释输入中的任意Skill/URL/owner。skill只走固定handler经核心调用，受每层契约与最小委托约束；微VM没有HostService、凭证或cgroup句柄。

微data点必须与原微input/output/side_effect一致。MPR1.3只向声明扩展点的微版本投影本次只读JSON；旧pure版本不因升级得到新桥。pure与network_enabled=false不能借扩展链或独立handler获取外部网络能力。

## 公共Skill与执行

两种模式的候选均由固定宿主点推导输入/输出/副作用，target.kind=extension固定版本、部署、package_digest和host_chain_digest。注册时核验来源与实际root安装；扩展候选不能伪装为宿主镜像原有定义。

用data候选调用宿主数据适配器取得完整符合Schema的结果，用skill候选调用固定handler。原宿主Skill的Schema、timeout及副作用不变；处理器输入输出或副作用不兼容时拒绝，不静默转换为更强执行能力。

验证所有层共享根deadline与取消链；校验、Prepare、排队、处理和等待不刷新超时。无匹配或合法空结果依Schema返回；非法输出、资源超限及部分stdout不得伪装为完整成功。

## 资源、故障和重放

核实扩展加载开销计入实际宿主根，不按链层复制节点预算或微250mCPU/256MiB。MPR父默认4vCPU/4GiB及实例4活动槽保持；独立handler另按原部署准入，不能忽略其计量。

微根执行仍必须通过核心Attach把停等worker实际迁入正确资源组后才运行。验证cgroup CPU/内存耗尽、组OOM、多Call共享及资源回收；MPR自报退出不能代替核心观测，扩展不获得宿主cgroup写权。

对有外部副作用的handler注入发送前失败、上游接受后断线、结果提交前崩溃、重复请求和取消。证据不足保持unknown，不换扩展/handler/幂等键自动重做；纯计算重算仍固定原链和期限。

## 静态规格检查

机器Schema、正反结构样例、包/来源摘要、typed宿主链摘要、HTTP/RPC/内部接口定义、权限目录及Markdown链接必须自洽。静态检查只能证明声明一致；真实data适配器、MPR投影、跨层授权和故障恢复应在实现验收中分别执行并保留结果。
