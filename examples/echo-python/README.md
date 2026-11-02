# 第二个独立普通插件实现

Python 服务直接实现现有 `PluginLifecycle` 和 `CapabilityService`，不导入 Rust 内核或私有 SDK。它与 Rust 示例共用 `org.xelab.echo@1.0.0` 的业务契约、文件字节和摘要；插件身份为 `org.xelab.echo-python`。两者同时安装后，调用方只需切换 `provider_deployment_id`。

```sh
python3 examples/echo/build_artifact.py --publisher-id "$PUBLISHER_ID" \
  --output examples/generated-python --implementation python
docker build -f examples/echo-python/Dockerfile -t xelab-echo-python .
```

签名与安装流程沿用 `examples/echo/sign_artifact.py` 和内核 README。CI 在 containerd 中实际启动两个镜像、验证 mTLS、调用相同契约并切换提供者。Python 使用 Ubuntu 24.04 的 gRPC/Protobuf 包，绑定在镜像构建时由仓库 Proto 生成，运行时不下载依赖。

证书由内核原子轮换；Python 在新 TLS 握手时重新加载，并要求客户端 SAN 包含 `spiffe://xelab.local/kernel`。不提供额外 HostService、工作流、存储或扩展能力。
