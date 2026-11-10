# 制品一致性预检工具

`check_artifact.py` 是离线、只读、无密钥的预构建制品一致性预检，面向第三方插件贡献者。
它复用 `crates/kernel/src/artifact.rs` 的成员/摘要规则与 `examples/*/build_artifact.py`
的既有严格契约，不自造签名格式；不下载镜像、不执行任何内容、不修改任何文件。

## 用法

```bash
# 先用既有构建器生成清单与契约输入（示例；publisher UUID 仅为占位）
python3 examples/echo/build_artifact.py --publisher-id 00000000-0000-4000-8000-000000000001 --output /tmp/artifact

# 检查预构建目录，或检查未压缩 tar 打包（只读取成员，不解包到磁盘）
python3 tools/artifacts/check_artifact.py --path /tmp/artifact
python3 tools/artifacts/check_artifact.py --path /tmp/artifact.tar --json
```

退出码：`0` 一致；`1` 不一致或不支持（fail closed）；`2` 输入路径不存在，明确标注
`not-checked`（未检查），不视为通过。

## 检查范围

- `xelab/plugin.json`：≤1MiB 严格 JSON（拒绝重复键、非有限数、尾随内容、非法 UTF-8）；
  `manifest_version` 必须为 `"1"`，其他格式 fail closed；publisher UUID、单行 plugin_id/version。
- 每个 capability 的四个契约成员存在、严格 JSON、schema 与清单逐字段一致、
  `semantics.json` 的 side_effect 一致，并按内核同款算法重算 `schema_digest`。
- HTTP 路由/Skill 引用的 `xelab/http/openapi.json`（≤4MiB）存在且摘要一致，
  Skill 引用的 `http_operation_id` 必须存在；capability 型 Skill 不得引用漂移的契约。
- 成员安全路径：拒绝符号链接、绝对路径、`..` 穿越、重复 tar 成员、链接/设备型成员；
  tar 仅支持未压缩格式，成员数上限 4096。

## 明确不做

- **不做 Ed25519 签名验证**，报告恒为 `signature_verification: not-performed`。
  签名验证属于内核安装路径（受信发布者公钥），本工具不接收私钥、不宣称签名有效。
- 不替代内核准入校验（profile/Schema 编译）、OCI 镜像摘要验证或正式宿主验收；
  一致仅表示本地成员与清单自洽。

## 测试

```bash
python3 -m unittest discover -s tools/artifacts -p 'test_*.py' -v
```

夹具由 `examples/echo` 与 `examples/http` 的既有构建器离线生成，覆盖正常产物、
摘要不符、schema/side_effect 漂移、缺失成员、路径穿越、重复成员、符号链接、
非规范 JSON、超限成员、不支持格式与未检查路径。