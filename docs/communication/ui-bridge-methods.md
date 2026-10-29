# iframe消息桥逐方法定义

[消息封装](ui-bridge.md) · [完整嵌套Schema](../contracts/ui-bridge.schema.json)

> 规格版本：2.6.0；状态：待实现。表中未标“可选”的参数均必填；额外参数拒绝。

| method | params | 成功result与映射 |
|---|---|---|
| skill.call | binding_alias、input、idempotency_key；timeout_ms可选100..300000默认120000且不超过Skill上限 | SkillCallResult；POST /skills/{绑定UUID}/calls |
| batch.create | binding_alias、request(SkillBatchRequest)、idempotency_key | SkillBatch；POST /skills/{绑定UUID}/batches，202作为接收结果返回 |
| batch.get | binding_alias、batch_id(UUID) | SkillBatch；GET /skill-batches/{id} |
| batch.items | binding_alias、batch_id；limit可选1..200默认50、cursor可选 | SkillBatchItemPage；GET /skill-batches/{id}/items |
| batch.result | binding_alias、batch_id、item_id(1..80关联键) | SkillCallResult；GET /skill-batches/{id}/items/{item_id}/result |
| batch.cancel | binding_alias、batch_id、if_match(强ETag)、idempotency_key；reason可选1..512字符 | SkillBatch；POST /skill-batches/{id}/cancel |
| window.open | contribution_id(本插件贡献ID) | {view_id:UUID}；宿主重新发现同部署window，检查plugin.ui.open和窗口上限后打开 |

路径均相对于/api/platform/v1；iframe不提供路径本身。skill.call要求绑定modes含call；batch.*要求含batch。调用前重新核对贡献摘要及Skill定义摘要，失配停止通道；核心继续验证当前Skill/目标权限。

batch.create在iframe协议只允许run_as=user，禁止service_account_id；需要获批长期服务身份时在核心任务中心明确提交。batch.get/items/result/cancel核对批次为当前owner、固定Skill UUID/digest与本绑定一致；插件不能用其他批次ID突破页面白名单，即使壳本身拥有更宽管理权限。

上述batch查询仍需batch.read；取消需batch.cancel；创建需batch.create和skill.invoke及原目标动作。读取item.result还检查当前业务/数据读取权。白名单只收窄WebUI当前用户权限，不授予缺失权限。batch.resume不开放给插件iframe，用户在可信宿主任务中心明确重新授权。

batch.get/cancel成功体中的revision由宿主缓存为ETag；cancel传入if_match必须符合最近读取版本。412由后端错误返回，不能无视版本替用户取消新状态。取消终态不代表先前写入已撤回。

skill.call在deadline内最多等待300s；batch.create等管理请求10s。请求超时返回DEADLINE_EXCEEDED，等待结果由原幂等键/批次查询核实，不开启新的副作用意图。batch.result的409/410沿后端原错误呈现。

window.open仅允许同部署已批准window贡献，widget自己也可请求打开对应窗口；不能给URL或打开其他插件/内核登录地址。关闭/最小化/最大化统一由宿主外框操作，页面不拥有全局窗口管理API。

## 宿主事件

| event/type | params | 语义 |
|---|---|---|
| ready（iframe→宿主） | 无 | 完成初步通道识别，禁止夹带令牌 |
| host.init（宿主→iframe） | theme_tokens、reduce_motion、reduce_transparency、visible、viewport{width,height}、locale、contribution_digest、bindings | 授权检查后启用页面桥；bindings是实际白名单 |
| theme.changed | theme_tokens、reduce_motion、reduce_transparency | 局部主题更新，不重载/重放请求 |
| visibility.changed | visible:boolean | 停止/恢复组件周期请求，最小化也为false |
| viewport.changed | width、height：1..8192逻辑像素 | 内容区变化，页面响应式布局 |
| widget.refresh | 空对象 | 可见组件按策略刷新已批准只读状态，不是业务写入授权 |
| host.dispose | reason:close/logout/upgrade/unavailable | 通道关闭，不取消已接受batch |

示例：[握手](../contracts/examples/ui-bridge-ready.json)、[batch请求](../contracts/examples/ui-bridge-batch.json)。业务input还需匹配实际绑定Skill Schema，通道结构验证不替代业务验证。
