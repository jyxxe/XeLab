# `ai.chat` 字段契约草案

[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)

> 规格版本：2.6.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。

能力 ID：`ai.chat`；操作目录：`chat`。

下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。

## 输入

[input.schema.json](input.schema.json)。

完整字段定义引用 [ChatRequest](../../../../contracts/openapi.json#/components/schemas/ChatRequest)。

## 输出

[output.schema.json](output.schema.json)。

完整字段定义引用 [ChatResponse](../../../../contracts/openapi.json#/components/schemas/ChatResponse)。

## 业务流

[stream.schema.json](stream.schema.json)。

完整字段定义引用 [ChatChunk](../../../../contracts/openapi.json#/components/schemas/ChatChunk)。

## 执行约束

[完整执行语义](semantics.json)。

| 项目 | 定义 |
|---|---|
| 副作用分类 | `non_repeatable` |
| 最大输入 / 输出 | 4194304 / 16777216 字节 |
| JSON 深度 | ≤64 |
| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |
| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |
| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |

补充语义与跨字段约束：

- Only existing platform text subset; upstream billing is not automatically idempotent.
- Stream frames retain ordering/backpressure/cancel; no retry after output starts.

落地门槛：`external-ai-adapter`、`profile-configuration`；见[门槛说明](../../../planned/feature-gates.md)。

四文件草案摘要：`sha256:e8d7b997ead0e149613ecc7f9fdccb6d954a1ed5af1ab2ccdc45861384367a3d`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。
