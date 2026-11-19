# C++ SDK 与跨语言一致性测试

[插件目录](README.md) · [启动环境](bootstrap.md) · [RPC 语义](../communication/grpc.md)

`sdk/cpp` 从仓库的 Proto 生成 C++ 的 PluginLifecycle、CapabilityService、HostService、DataMigration 和 TaskWorker 绑定。插件可链接 CMake 目标 `xelab_plugin_sdk` 并引入 `<xelab/sdk.h>`。参考插件复用 Rust Echo 的一元和流式契约文件。[C++ Echo 源码](../../examples/echo-cpp/main.cc)

## 构建和实现

在 Ubuntu 24.04 安装 C++17 编译器、CMake、Protobuf、gRPC C++、OpenSSL、nlohmann-json 和 bubblewrap。

```sh
sudo apt-get install g++ cmake make libprotobuf-dev libgrpc++-dev \
  protobuf-compiler-grpc libssl-dev nlohmann-json3-dev bubblewrap
cmake -S sdk/cpp -B target/cpp-sdk -DCMAKE_BUILD_TYPE=Release
cmake --build target/cpp-sdk --parallel
cargo build -p xelab-kernel --bin xelab-echo --locked
```

在插件项目中，可用 `add_subdirectory()` 引入 `sdk/cpp` 并链接 `xelab_plugin_sdk`。
作为子目录引入时默认只构建库。独立配置 `sdk/cpp` 时默认启用 Echo 和一致性客户端，它们需要 nlohmann-json。
用 `XELAB_SDK_BUILD_TOOLS` 控制参考程序是否构建。启用后，`BUILD_TESTING` 控制一致性客户端是否构建。

为插件实现生成的生命周期和能力服务，使用 `ConfigurePluginServer` 注册固定 mTLS 监听器。SDK 核验工作负载 CA 与内核 SPIFFE URI，限制单条 gRPC 消息大小，并通过证书文件监视器更新新连接的证书。实现方仍需验证业务 Schema 和摘要，并协作处理取消、排空和流终帧。C++ Echo 支持一元及正常流式调用，Rust 专用故障夹具在 C++ 端返回参数错误。

`ConnectHost` 返回固定宿主入口的 HostService stub，核验内核 DNS 名称和 SPIFFE URI。`HostCallContext` 继承当前 gRPC 调用的取消状态，取传输期限、`RpcContext` 期限及方法上限的最小值。调用方使用当前委托填充 Proto 请求，并可从完成后的调用读取 `xelab-error-bin`。HostService 逐调用确认当前实例与委托。[HostService 方法](../rpc/README.md#hostservice)

## TaskWorker 绑定

`<xelab/sdk.h>` 导出生成的 `xelab::v1::TaskWorker`，含服务基类、客户端 stub 和检查点消息。独立 CMake 消费者测试通过实际 gRPC 回环连接核对进度流及其失败语义。端到端执行仍需内核调度接入、检查点持久化和跨重启去重。[Execute 契约](../rpc/TaskWorker.Execute.md)

`xelab::sdk::TaskProgressReader` 接受调用方已认证的工作负载通道、原 Execute 请求和绝对截止时间，只发起一次调用，期限还受请求上下文及 Execute 方法上限约束。调用方配置 `kMaxMessageBytes` 收发上限，并将通道的 `GRPC_ARG_ENABLE_RETRIES` 设为 `0`。`Next()` 校验执行 ID、fence 和从 1 开始的连续序号，分别返回心跳、检查点和终态。读取器先解析 protobuf 消息，读完流并取得 C++ gRPC `Finish()` 返回的 OK 状态之后才交付终态。

提前断流、额外消息、非 OK 状态和超时均保留结果不确定性。底层 HTTP/2 与 gRPC 记录的处理以 C++ gRPC 暴露的消息和状态为准。在 gRPC 1.51 的回环实验中，终态后的残缺记录在 OK 尾部状态下被框架折叠成流结束。`TaskUnknown` 表示实际结果仍待核实。成功终态的输出受 16 MiB 和 UTF-8 限制，调用方仍需验证业务 JSON、输出 Schema 和错误码，并按检查点归属、版本及持久化约束处理恢复。

## 离线一致性测试

```sh
python3 tools/acceptance/sdk-conformance.py \
  --rust target/debug/xelab-echo \
  --cpp target/cpp-sdk/xelab-echo-cpp \
  --client target/cpp-sdk/xelab-sdk-conformance
```

同一个生成的 RPC 客户端在隔离回环网络中驱动两个参考二进制。脚本核对两端的原始契约，使用临时 CA 测试内核身份、生命周期、输入校验、期限、取消、流终帧和错误。C++ HostService 客户端另有 mTLS、委托拒绝及父调用取消传播的回环测试。本地使用 Linux 用户命名空间，CI 使用仅有回环接口的 Docker 容器。

## 签名 OCI 验收

CI 在独立的 Ubuntu runner 构建最终镜像，按平台 manifest 的不可变摘要导入 containerd。验收通过平台 API 拒绝绑定到其他摘要的签名，再安装正确签名的镜像。启用获批的部署后，经 mTLS 执行一元能力调用和 Skill SSE 流式调用，并核对调用记录对应同一个容器实例。停用部署后检查容器已移除，CPU 和内存预留已释放。[验收工作流](../../.github/workflows/plugin-sdk.yml)

## 生成制品并提交

用实际发布者 UUID 替换 `PUBLISHER_ID`，生成清单和契约后构建镜像。

```sh
python3 examples/echo/build_artifact.py --implementation cpp \
  --publisher-id "$PUBLISHER_ID" --output examples/echo-cpp/generated
docker build -f examples/echo-cpp/Dockerfile -t docker.io/local/xelab-echo-cpp:1.0.0 .
```

取平台对应的不可变 OCI manifest 摘要，按制品文档签名并走现有发布者信任、安装和批准流程。提交 PR 时记录制品版本、共用契约摘要、离线测试和单独执行的签名 OCI 验收。[制品与签名](artifacts.md) · [仓库贡献流程](../../CONTRIBUTING.md)
