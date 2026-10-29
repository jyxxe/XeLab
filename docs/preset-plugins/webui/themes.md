# 可变主题与用户偏好

[WebUI目录](README.md) · [偏好接口](../../api/operations/ui/README.md)

> 规格版本：2.6.0；状态：待实现的规范。

主题有system、light、dark、custom四种模式。system跟随浏览器prefers-color-scheme；light/dark固定模式；custom必须提供完整UiThemeTokens，其他模式禁止带custom_tokens。用户可以即时预览，确认后用ETag保存，取消则恢复已存偏好。

主题由有限角色令牌构成，不加载任意主题脚本、外部CSS、远程字体或背景URL。核心验证字段类型/颜色格式，壳验证显示可读性；不适用的组合提示调整并保留可恢复默认主题入口。

| 令牌 | 范围与用途 |
|---|---|
| accent | 交互强调；默认light为#4F7CFF、dark为#80A7FF |
| desktop / surface | 桌面与窗口面板背景 |
| text / muted_text | 主文本与次级文本，不能只降低透明度造成不可读 |
| border / danger | 边界与危险状态；错误同时有文字/图标 |
| radius_px | 0..24，窗口和面板圆角 |
| blur_px | 0..32；不支持模糊时退化为实色/透明面板 |
| panel_opacity | 0.6..1；reduce_transparency时实际强制1 |
| font | system/sans/serif/mono，映射宿主内置字体栈 |

light默认令牌：desktop #E9EEF7、surface #FFFFFF、text #18202C、muted_text #526074、border #CBD5E1、danger #B42318、radius=12、blur=16、opacity=0.92、font=system。

dark默认令牌：desktop #121826、surface #1F2937、text #F3F4F6、muted_text #B4C0D0、border #475569、danger #FF8A80，其余默认与light一致。resolved_tokens包含当前有效完整值，由宿主经host.init/theme.changed发送给iframe；插件不能直接依赖父页面CSS变量跨源读取。

reduce_motion与系统prefers-reduced-motion任一为true时减少动画；reduce_transparency禁用模糊并设面板不透明。切换主题不重载iframe、不重发业务请求、不改变数据权限；插件页面需使用收到的令牌更新自己的局部样式。

GET /me/ui-preferences读取本人记录，首次默认system/底部Dock/空组件；PUT /me/ui-preferences携If-Match及Idempotency-Key完整替换。部署default_theme只作为壳在首次GET之前的加载回退；核心已存偏好（含默认system）优先，管理员不能通过替换WebUI清空用户选择。

偏好由核心按账户保存，资源类型account，动作ui.preferences.read/update，API Key仍受其固定上限。只允许本人入口；窗口矩形保存在同用户浏览器本地，跨用户切换必须清空。服务器偏好不保存密钥、HTML、业务输入或批次结果。
