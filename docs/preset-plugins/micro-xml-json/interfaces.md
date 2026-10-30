# convert.xml-json接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/xml-json/input.schema.json) |
| 输出 | [output.schema.json](contracts/xml-json/output.schema.json) |
| 流式 | [stream.schema.json](contracts/xml-json/stream.schema.json) |
| 语义 | [semantics.json](contracts/xml-json/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `direction` | `xml-to-json`或`json-to-xml` |
| 输入 | `data` | 前者为XML字符串，后者为一个根元素JSON树 |
| 输出 | `direction` | 实际转换方向 |
| 输出 | `data` | 前者为根元素JSON树，后者为完整UTF-8 XML字符串 |

## 固定树映射

每个元素为`{name,namespace_uri,prefix,namespaces,attributes,children}`。name为不含冒号的local name；namespace_uri与prefix均为字符串或null，无命名空间时为null。namespaces为本元素声明的`{prefix,uri}`数组，默认命名空间prefix为null。

attributes为按输入顺序保留的`{name,namespace_uri,prefix,value}`数组，命名空间声明只放在namespaces中。无前缀属性不继承默认命名空间；重复的展开属性名`(namespace_uri,name)`无效。保留xml前缀的标准绑定并拒绝重新绑定。

children为有序数组，每项是文本字符串或递归元素对象；同名重复元素保留为多项，不折叠或自动猜类型。纯数字文本仍是字符串；空元素children为空数组。相邻文本片段合并，CDATA映射为普通文本，空白按XML规范的换行处理后保留。

XML注释不进入JSON；处理指令拒绝，XML声明仅允许1.0/UTF-8。DTD、外部实体、一般自定义实体、外部Schema、XInclude及网络扩展均拒绝；只解码预定义实体和合法数字字符引用。

json-to-xml验证合法XML名称、字符、唯一属性和前缀绑定，所有文本及属性由序列化器按上下文转义。禁止原始XML插入，不能靠字符串拼接将JSON值绕过转义。根节点必须恰好一个，树结构与方向不匹配整次失败。

## 完整性与预算

树最多10000元素、每元素128属性；输入输出JSON深度64及1MiB字节预算同时生效。JSON对象层和children数组也计算深度，不能把XML深度64当成额外独立额度。

往返保证上述数据模型的结构语义，不保证注释、CDATA分界、原属性引号、空元素写法或原字节一致。超限、解析器不可用和非法结构均整次失败。无流式结果，多份文档使用核心异步batch。
