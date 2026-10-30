# 官方WebUI预设：可变主题桌面

[文档总目录](../../README.md) · [可信登录边界](../../plugins/webui.md)

> 规格版本：2.8.0；状态：设计预设，页面、镜像和交互尚未实现或实测。

org.xelab.webui是独立OCI插件。建议0.5vCPU/512MiB、临时盘256MiB、活动HTTP32，消耗插件池；浏览器进程的内存不算服务器容器用量。内核至少4C16G预留不受界面启停影响，最终UI资源需实测。

界面采用类似macOS的桌面交互：顶部菜单栏、桌面、Dock、可拖动缩放的应用窗口和桌面组件，支持system/light/dark/custom主题。视觉元素使用自有图标与令牌；可替换主题，不把布局绑死在一种配色上。

| 文档 | 内容 |
|---|---|
| [桌面与导航](desktop.md) | 布局、Dock、应用启动、键盘和窄屏 |
| [窗口管理](windows.md) | iframe窗口、焦点、层级、最小化与恢复 |
| [桌面组件](widgets.md) | 网格、挂载、刷新、权限与资源 |
| [主题与偏好](themes.md) | 可变主题令牌、系统跟随、持久化 |
| [插件页面接入](plugin-ui.md) | 发现、固定版本、窗口/组件生命周期 |
| [消息桥](../../communication/ui-bridge.md) | 每个消息与方法定义、授权和错误 |
| [管理应用](management.md) | 插件、Skill、账户与知识表界面 |
| [异步任务中心](batches.md) | batch提交、进度、逐项结果与取消 |
| [验收](acceptance.md) | 桌面/隔离/主题/恢复的可验证条件 |

[清单](manifest.json)、[默认配置](defaults.json)、[部署模板](deployment.template.json)给出机器字段。默认页面desktop，默认主题system，最多16个打开窗口、8个活动组件，刷新下限30s；这些是初始策略，不是已测性能承诺。

此预设不发布业务Skill；以当前用户授权消费核心管理API和插件Skill。每个业务插件可选择提供iframe窗口、iframe桌面组件或二者，统一遵守[UI贡献规范](../../plugins/ui-contributions.md)。

仅登录壳/静态GET/HEAD是批准的public路由，页面无私有数据；登录和授权页由内核可信源提供。全部管理/业务请求另行认证。停用WebUI不影响管理API、CLI或已接受的异步批次。
