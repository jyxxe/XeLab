# Markdown 转 HTML 接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。

## `convert.markdown-html` / `markdown-html`

[输入 Schema](contracts/markdown-html/input.schema.json) · [输出 Schema](contracts/markdown-html/output.schema.json) · [流式声明](contracts/markdown-html/stream.schema.json) · [行为契约](contracts/markdown-html/semantics.json)

输入为 `{markdown: string}`，输出为 `{html: string}`。v1 不接受用户指定解析器、清理器、脚本扩展或任意允许规则。

按不可变制品声明的 Markdown 方言解析，再对生成的 HTML 做允许清单清理；原始 HTML 不直接穿透。清理必须删除 `script`、内联事件、`iframe`、`object`、`embed`、可执行 SVG/MathML、`srcdoc` 及危险样式/属性。未在允许清单中的标签不得保留活动语义。

URL 必须在实体与转义归一后校验；链接只保留 `http`、`https`、`mailto` 或不引入协议的相对地址，拒绝 `javascript`、`data`、`vbscript` 等执行/嵌入形式。图片等资源不在转换时获取，消费方决定后续网络访问策略。外链新窗口行为若被添加，必须带隔离 opener 的属性。

Markdown 内容、代码块和文本节点按上下文转义，避免解析前后产生清理旁路。清理规则和方言必须随制品版本固定，升级不得复用旧摘要冒充同一实现。

空 Markdown 返回空 HTML；不完整 Markdown按固定方言作为可解析文本处理。解析器内部错误、清理失败、输出超限或时间/内存越界均整体失败，不返回未清理或部分 HTML。

v1 非流式，一次返回完整 JSON。调用成功不构成 `trustHTML` 授权；WebUI 等消费方仍应按来源、iframe 沙箱及自身渲染策略处理该字符串。
