# ID列表链式表达式与参数绑定

[知识库目录](README.md) · [列表与标量函数](query-functions.md) · [执行快照](query-execution.md) · [自动缓存](query-cache.md)

> 规格版本：2.8.0；状态：待实现的设计契约；rows-query使用2.0.0版本。本文不表示已有解析器或查询服务。

## 每个表达式返回完整ID列表

根变量`a`表示当前表的一份有序、去重的行UUID列表。省略input_ids时为当前读快照全部live行，按row_id升序；显式input_ids绑定给定列表，保持输入顺序，空数组就是空列表，不能解释为全表。

每个列表操作都返回完整列表，作为下一阶段的输入；最终响应ids也完整返回，不使用limit、cursor或隐式截断。显式输入中不存在、已删除或不属于当前表的ID使整次请求失败，不悄悄漏掉行。根列表、中间阶段和最终列表均最多100000个ID，超限明确报错。

重复input_ids拒绝为INVALID_ARGUMENT，不自动去重后改变调用方输入；所有正常输出天然保持唯一ID。

```text
a
a.match(len(title) >= 7 && len(content) >= 100)
a.match(len(title) >= 7 && len(content) >= 100).topk(content, :q, 10).match(contains(title, :term))
a.topk(content, :q, 100).topk(title, :title_query, 10)
```

`match`只过滤当前列表，保持原顺序；`topk`只给当前候选评分并重排。执行严格从左到右，`topk(...).match(...)`不能改写为`match(...).topk(...)`，后置过滤剩余不足k条时不回补。

参数为`{name:{type,value}}`，例如term为string，q/title_query为vector_query。参数名不含冒号，使用前必须绑定，未使用参数拒绝；字符串不能拼接成SQL或表达式片段。

## 列表语法

```ebnf
expression = "a", { stage } ;
stage      = ".match(", predicate_expr, ")"
           | ".topk(", identifier, ",", parameter, ",", int_arg,
             [ ",", number_arg ], ")" ;
int_arg    = integer | parameter ;
number_arg = finite_decimal_number | integer | parameter ;
```

根变量和方法名采用上列精确小写拼写；只能调用match/topk，最多32个列表阶段，其中topk最多8个。点语法仅用于这些列表阶段，不允许任意成员访问、动态方法名或可执行脚本。

## match中的标量谓词

```ebnf
predicate_expr = or_expr ;
or_expr        = and_expr, { ("or" | "||"), and_expr } ;
and_expr       = not_expr, { ("and" | "&&"), not_expr } ;
not_expr       = ("not" | "!"), not_expr | predicate ;
predicate      = value, [ comparator, value | "in", "(", value, {",", value}, ")" ] ;
comparator     = "==" | "!=" | "<" | "<=" | ">" | ">=" ;
value          = identifier | parameter | literal | scalar_call | "(", predicate_expr, ")" ;
scalar_call    = name, "(", [value, {",", value}], ")" ;
parameter      = ":", name ;
identifier     = name | double_quoted_identifier ;
literal        = integer | finite_decimal_number | single_quoted_string | "true" | "false" | "null" ;
```

name为`[A-Za-z][A-Za-z0-9_]*`；谓词关键词和标量函数名大小写不敏感，列名大小写敏感。单引号字符串用双单引号转义，双引号标识符用双双引号转义；不解释反斜杠，不允许原始换行。任意长文本/控制字符通过参数绑定。

`field in (:allowed)`允许一个typed array参数展开，仍执行1000项上限及元素类型检查；不能混用array参数和其他列表值。标量函数仅允许[明确签名](query-functions.md)；topk不能放在match、OR、NOT、比较或其他标量函数内部。

比较NULL产生unknown，match只保留true；检查空值使用is_null。AND/OR/NOT遵循三值逻辑，不把NULL当0或空串。裸非bool值不能作为match谓词。

表达式UTF-8≤8192字节、AST≤256节点、深度≤32、IN≤1000项。无SELECT/FROM、SQL子查询、赋值、分号、多语句、注释或任意代码。此语法替代旧版顶层布尔filter，旧版`len(...) and topk(...)`不能在新契约中隐式兼容。

## 在请求之间复用列表

客户端可把响应ids作为后续请求的input_ids，再执行`a.match(...).topk(...)`。这是按值提供候选ID，读取后续请求的当前快照，不是持久服务器变量、旧快照或授权凭证。

若要求后续查询仍处于原schema/data水位，必须成对传expected_schema_revision和expected_data_revision；失配返回KB_QUERY_SNAPSHOT_EXPIRED。向量索引内容可能在相同数据水位下继续更新，各topk阶段仍返回实际index_revision。它们是该次查询的历史执行说明，不承诺跨请求保留旧向量版本，不能把行水位当成向量冻结证明。
