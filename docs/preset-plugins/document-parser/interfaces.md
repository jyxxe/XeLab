# 文档解析接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.8.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## `document.parse`

| 输入字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `blob_id` | UUID，必填 | 当前调用主体有权读取的核心 Blob，≤64MiB |
| `format` | `auto/pdf/docx/txt/html`，默认auto | auto先检查签名与内容；不只信任扩展名 |
| `options.include_pages` | boolean，默认true | 要求可用的页面定位 |
| `options.max_pages` | integer，1..1000，默认200 | 解析页数硬上限；超出拒绝，不输出伪完整结果 |

| 输出字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `format` | `pdf/docx/txt/html` | 检测并使用的格式 |
| `text` | string | 完整解析正文，统一为UTF-8和LF换行 |
| `pages` | array | 项含 `number/text`；number从1开始，仅真实页概念可提供 |
| `paragraphs` | array | 项含 `index/text/page_number`；index从0开始，无法定位时page_number为null |
| `metadata` | object | `title/author/language/page_count`，未知值为null |
| `warnings` | string数组 | 有界、去除敏感路径的解析提示 |

TXT、HTML及未分页DOCX不捏造物理页号；页面不可得时 `pages=[]`、`page_count=null`。关闭页输出时pages为空；paragraphs与正文仍为完整结果。元数据来自文档内容，不能作为可信身份或权限声明。

## 文件安全与失败

解析器不能访问任意本地路径、URL或其他用户文件。HTML不取外部CSS、图片和脚本；不执行脚本、宏、嵌入程序。DOCX等内部归档同样校验解压体积、数量和深度，拒绝路径逃逸、炸弹及加密而不可解析的文档。

实际内容与指定格式冲突、损坏或不支持的加密文档返回 `INVALID_ARGUMENT`。页数、时间、内存或JSON输出上限不足时返回统一资源/期限错误；不静默省略尾页。批量解析走异步batch，单项输出仍有界。

## 原始契约

- `document.parse`： [输入](contracts/parse/input.schema.json) · [输出](contracts/parse/output.schema.json) · [流](contracts/parse/stream.schema.json) · [语义](contracts/parse/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

