# 版本、能力契约与扩展

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 三种版本

插件版本使用 SemVer 2.0.0；平台管理 API 主版本固定在 `/api/platform/v1/`；能力契约以 name/version/schema_digest 独立标识。三者不能推导或替代。

本版版本约束仅支持精确 `x.y.z` 或空格连接的 `>=x.y.z <a.b.c` 交集。禁止隐式 caret、tilde、OR、通配符；预发布版本必须显式精确指定。

## 契约包

制品内 `/xelab/contracts/<name>/<version>/` 保存 input.schema.json、output.schema.json、stream.schema.json（无流时为 JSON null）、semantics.json。semantics 包含 side_effect、错误码、取消、幂等和结果查询语义。

摘要按固定文件顺序分别取原始文件 SHA-256，拼接 `<filename> <sha256hex>\n` 后再次 SHA-256。输入输出 Schema 的修改也必须产生新契约版本，不能以“JSON 兼容”名义覆盖同版本 digest。

不兼容字段或语义变化升级主版本；兼容新增可选字段可升级次版本，但调用仍锁定明确版本与 digest。插件同时声明多个契约版本才能承接不同版本消费者。

## 接入边界

注册能力不是把任意 JSON 通道开放给网络。安装时验证 Schema，路由前验证请求，输出时验证响应，错误和流式块也执行上限约束。新增实现沿用契约无需改内核；新契约必须附验证样例与副作用说明。

标准能力名示例：ai.text-generation、ai.embedding、ai.reranking、document.parse、web.search、storage.object、automation.execute。实际提供哪些能力由签名清单决定，不保证所有 AI 插件支持全部能力。
