# 外部规范与实现依据

[文档总目录](../README.md)

以下为所采用协议和机制的一手规范。平台自己的范围、默认值与授权继承规则属于本项目设计，不能声称由这些规范自动保证。

- [OpenAPI 3.1.1](https://spec.openapis.org/oas/v3.1.1.html)：HTTP 契约版本。
- [JSON Schema 2020-12](https://json-schema.org/draft/2020-12/json-schema-core)：数据约束语义。
- [Proto3 语言](https://protobuf.dev/programming-guides/proto3/)：字段号、出现性、oneof、版本兼容。
- [gRPC 取消](https://grpc.io/docs/guides/cancellation/)及[重试](https://grpc.io/docs/guides/retry/)：期限、取消和提交后的重放边界。
- [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html)：HTTP 方法、条件请求与状态语义。
- [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html)：幂等请求指纹采用的 JCS 规范化。
- [RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html)：OAuth 安全、PKCE、令牌范围。
- [RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html)：主体与代理执行者、受限委托模型参考。
- [OWASP 授权](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html)：默认拒绝、对象校验及逐请求授权。
- [OWASP 密码存储](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)：Argon2id 参数基线。
- [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)：CPU 时间预算、内存限制/保护、进程和 I/O 控制。
- [containerd scope](https://containerd.io/scope/)：网络、外部卷和日志仍需上层集成。
- [浏览器同源策略](https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Same-origin_policy)：路径前缀不能替代独立来源隔离。

资料核对日期：2026-10-28。未来升级依赖或规范时，应按变更流程核对契约差异，而不是自动跟随 latest 改变已发布行为。

EmbeddingGemma 2官方依据：[模型卡](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2)、[官方仓库](https://huggingface.co/google/embeddinggemma-2)。只从中采用模型名、模态、维度/归一化与数值精度事实；CPU/内存/并发为本项目预设而非官方性能保证。
