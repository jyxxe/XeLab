# PostgreSQL 连接器接口规划

[插件目录](README.md) · [预设目录](../README.md) · [Skill转发](../../skills/definition.md)

> 规格版本：2.6.0；状态：`planned`；计划业务契约版本：0.1.0，尚无可调用服务。

业务通过核心Skill调用；下列名称是能力ID，不是新增HTTP或RPC路径。输入拒绝未知字段，权限、根deadline、取消和批处理复用核心协议。JSON输入≤4MiB、输出≤16MiB；文件字节只通过有权访问的核心Blob传递，每个Blob≤64MiB。

## 公共输入

| 字段 | 类型 / 约束 | 含义 |
|---|---|---|
| `connection_alias` | string，必填 | 固定数据库、角色、网络与Secret绑定 |
| `sql` | string，必填 | 一个允许的参数化SQL语句，不允许多语句拼接 |
| `parameters` | 至多128项的`{type,value}`数组，可省略 | 顺序绑定 `$1,$2,...`，不能字符串替换；省略时不绑定参数 |
| `timeout_ms` | 100..30000，默认10000 | 连接/执行/读取总限时，受根deadline收窄 |

输入不能覆盖DSN、用户名、密码、数据库角色或搜索路径。连接池按固定配置隔离，归还前清理事务和会话状态；不允许跨Skill调用保留隐式事务。

每个参数拒绝未知字段，type与value必须匹配；不能根据JSON外观猜测SQL类型。数值和二进制编码在驱动绑定前校验，不拼接进SQL文本。

| `type` | `value`编码 |
|---|---|
| `null` | JSON null，表示SQL NULL |
| `boolean` | JSON boolean |
| `int64` | 有符号64位整数的十进制字符串；不使用JSON数承载大整数 |
| `decimal` | 无指数的十进制定点字符串，精度/scale受批准列与驱动限制 |
| `float64` | 有限JSON number，拒绝NaN、Infinity及溢出 |
| `text` | Unicode字符串，不隐式转为数字或日期 |
| `bytes` | 规范base64字符串，仅为有界SQL值，不授予任意文件读取能力 |
| `uuid` | 规范UUID字符串 |
| `timestamp` | UTC RFC3339时间字符串 |
| `json` | 有界I-JSON值；JSON null与type=null的SQL NULL语义不同 |

## `database.query`

额外输入 `max_rows` 为1..10000整数，默认1000。仅接受批准的只读SELECT；WITH等结构也必须证明无写入、无危险函数调用，不能只检查首个单词。数据库只读事务和角色权限作为独立防线。

| 输出字段 | 类型 | 含义 |
|---|---|---|
| `columns` | array | 有序的 `name/data_type` 元数据；允许重名列 |
| `rows` | 二维数组 | 每行位置与columns一致；按下述PostgreSQL列类型编码 |
| `row_count` | integer | 本次返回行数，等于rows长度 |
| `truncated` | boolean | 有更多结果未返回；不能声称完整 |
| `elapsed_ms` | 非负integer | 插件观察到的执行耗时 |

结果不依赖运行时自动猜测：SQL NULL映射为null，bool为boolean，int2/int4为整数，int8和numeric始终为十进制字符串，float4/float8为有限JSON数，text/UUID/日期时间为固定格式字符串，bytea为规范base64字符串，json/jsonb为有界I-JSON值（可含数组/对象）。因此rows元素不限于标量；列data_type决定解释方式。

未支持的数组/复合类型、非法浮点或不可无损转换的值拒绝，不静默变为null或截断数字。每行长度须等于columns数量，row_count须等于rows数量。查询没有跨次一致分页承诺；需稳定分页时由SQL明确排序与边界。

## `database.execute`

仅接受批准的单条INSERT、UPDATE或DELETE；本版不提供DDL、COPY、事务控制、角色切换和任意管理SQL。完成一次明确提交后输出`affected_rows`（0..9223372036854775807的无符号十进制字符串）和`elapsed_ms`；affected_rows不使用JSON数，以免大整数丢失精度。不隐式返回任意RETURNING结果集。

平台幂等记录避免已知成功的重复执行，但不能独自保证远端数据库恰好一次。连接断开或超时发生在提交窗口时必须记录结果不明，不自动重试；需要业务唯一键、目标端去重表或人工核实后解决。

## 限额与隔离

为语句设置行数、返回字节、statement_timeout及锁等待上限；禁止未经批准的外部函数、文件和网络扩展。SQL审计脱敏参数和连接秘密。查询取消应撤销当前语句；取消并不证明写入从未提交。

## 原始契约

- `database.query`： [输入](contracts/query/input.schema.json) · [输出](contracts/query/output.schema.json) · [流](contracts/query/stream.schema.json) · [语义](contracts/query/semantics.json)
- `database.execute`： [输入](contracts/execute/input.schema.json) · [输出](contracts/execute/output.schema.json) · [流](contracts/execute/stream.schema.json) · [语义](contracts/execute/semantics.json)

本页操作不提供业务结果流；batch进度仍使用核心异步协议。原始契约的gate仅描述待落地条件，不使未实现依赖变成现有能力。

