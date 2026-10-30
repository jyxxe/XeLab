# 字段类型与JSON表示

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

## 标量与复合值

| kind | wire值 | 精确边界 |
|---|---|---|
| boolean | JSON bool | 不把0/1或字符串当bool |
| int32 | JSON integer | -2147483648..2147483647 |
| int64 | 十进制字符串 | -9223372036854775808..9223372036854775807；无空白/加号/无意义前导0 |
| float32 | JSON number | 有限IEEE754，写入转换为float32；不接受NaN/Infinity |
| float64 | JSON number | 有限IEEE754双精度 |
| decimal | 十进制字符串 | precision 1..38，scale 0..precision；超过精度或需要舍入时拒绝 |
| string | JSON string | 短文本，最多4096 Unicode码点 |
| text | JSON string | 最多1MiB UTF-8原始文本，整体请求仍受核心4MiB上限 |
| bytes | 标准Base64字符串 | 解码最多1MiB，校验padding和规范编码 |
| date | YYYY-MM-DD | 合法公历日期，不含时区 |
| timestamp | RFC3339字符串 | 必有时区，归一UTC；最多微秒6位小数 |
| uuid | UUID字符串 | 规范小写连字符形式 |
| json | JSON值 | 无重复key，有限数字，整数≤2^53-1，单元≤1MiB |
| media | parts对象 | 文本片段及同表media_id引用，见媒体规范 |
| array | JSON数组 | 0..1024项，element_kind为基本标量；不嵌套array/media，不允许null元素 |

单行规范JSON≤1MiB；单请求最多4MiB。外层列nullable与数组元素规则不同，不能用缺失、空字符串和null互相替代。null只有nullable=true时允许。

字符串保留原值，不隐式trim、NFC或locale转换；len按Unicode码点计数而非字节/字素。decimal不经过float中转，int64不靠JSON number冒险保存。

## 查询比较

int32/int64可精确提升到int64；float32/float64按float64比较；decimal只与同类decimal比较，文本string/text同类兼容。其他混合类型须参数与字段类型明确匹配，不做字符串转数字。

整数字面量按int64解析，小数字面量按float64解析；和明确float字段比较的整数字面量仅在可精确表示时转换。typed参数严格按其type校验，不因左值不同而重新解释。decimal/date/timestamp/uuid建议使用显式typed参数。

json/array/media只允许等值与支持它们的函数，不支持大小比较或任意排序。JSON等值按规范JSON结构，bytes按解码字节。所有类型错误为KB_QUERY_TYPE_ERROR，不能静默把数值当字符串排序。
