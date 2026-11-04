# 独立声明式 HTTP 参考插件

`org.xelab.http-echo` 在 `8080` 运行真实 Python 标准库 HTTPS 服务，在 `7000` 使用仓库 Proto 生成的 `PluginLifecycle`。声明的 POST 操作包括 `echo`（返回文本）、`hold`（有界延迟）和 `events`（真实 StreamEvent SSE）；静态页面仅支持 `GET/HEAD /ui/org.xelab.http-echo/index.html`。不提供任意 URL、文件路径、Cookie、重定向或 WebSocket。

`openapi.json` 的**原始文件字节** SHA-256 固定到清单 HTTP 路由及候选 Skill；操作 `x-permission` 与 `RouteSpec.required_action` 相同。候选 Skill 不会因安装自动注册或启用。清单及 OpenAPI 必须和镜像一同签名。

当前参考插件用于真实传输、静态安全及后续 OCI 网关集成验收。仅运行下面的 Python 测试不能证明内核已经开放 HTTP 路由、实现 HTTP Skill、完成 OCI 生命周期或交付 M3-B；这些需要内核集成和相应真实主机测试。

```sh
python3 -m unittest discover -s examples/http -p 'test_*.py' -v
python3 examples/http/build_artifact.py --publisher-id "$PUBLISHER_ID"
docker build -f examples/http/Dockerfile -t docker.io/local/xelab-http-echo:1.0.0 .
python3 examples/http/container_probe.py --image docker.io/local/xelab-http-echo:1.0.0
docker save docker.io/local/xelab-http-echo:1.0.0 -o examples/http/generated/http.tar
sudo ctr -n xelab images import examples/http/generated/http.tar
```

`--include-rpc --plugin-version 2.0.0 --output examples/http/generated-mixed` 生成另一个签名版本，提供实际 `org.xelab.http-echo.rpc@1.0.0` 能力和四文件契约包。RPC 与 HTTP 共用应用的四个物理业务槽；默认 HTTP-only 版本没有 capability，不通过虚假 provider 获得健康状态。混合版本的容器探针同时占用三个真实 RPC 和一个 HTTP 请求，验证第五个请求被拒绝。

从 `ctr images list` 取得平台对应的 OCI image manifest 摘要，以现有签名工具生成安装请求；不能使用多平台 index 摘要或可变 tag 签名：

```sh
sudo ctr -n xelab images tag docker.io/local/xelab-http-echo:1.0.0 \
  docker.io/local/xelab-http-echo@sha256:<platform-manifest-digest>
python3 examples/echo/sign_artifact.py \
  --manifest examples/http/generated/xelab/plugin.json \
  --image docker.io/local/xelab-http-echo@sha256:<platform-manifest-digest> \
  --key publisher.key > examples/http/generated/install.json
```

`container_probe.py` 为 UID 65532 的真实镜像执行 `--network none` 组件测试：临时测试 CA、真实 gRPC 初始化/排空/恢复、HTTPS 业务、SSE 与静态请求、错误工作负载拒绝。该测试将仅供测试的 CA/叶证书包只读挂载给测试程序，不验证内核颁证、进程隔离、签名准入或内核 OCI 生命周期；这些仍须独立集成验收。特权 `tests/http.rs` 与 CI 的独立 HTTP OCI 步骤负责实际内核接入，未执行的部分不能由组件探针代替。

安装、批准、部署及启停沿用公开管理 API；网关只连接运行时观测到的实例地址。HTTP 固定监听 `0.0.0.0:8080`，证书采用现有 `XELAB_TLS_CERT_PATH/XELAB_TLS_KEY_PATH/XELAB_TLS_CA_PATH`；每次 TLS 握手重新加载证书，并校验平台 CA 及精确内核 SPIFFE SAN。业务请求还需内核重建的 `X-Xelab-Delegation`、UUID `X-Xelab-Subject/Call-ID`、十进制 `X-Xelab-Deadline-Unix-Ms` 和 `X-Xelab-Idempotency-Key`（UTF-8 业务键的无填充 base64url），不接受用户原始 Bearer。委托是不透明句柄，插件不解释角色，也不实现平行用户授权。

业务请求在 `Initialize` 后接受，在 `Drain` 后拒绝；实例应用额外限制四个同时活动业务请求。该局部限制不代替内核所有入口共享的 Broker 并发与配额。GET/HEAD 静态壳不需要用户凭证，但后端连接仍须内核 mTLS。测试使用临时 CA 和真实回环 HTTPS，覆盖正确内核、其他工作负载、缺证书、委托、正文、路径和停用状态；不使用 AWS。


签名参考制品还提供 GET `/read`、PUT/PATCH `/created` 和 DELETE `/discard`，
验证固定方法、201 JSON 和 204 无正文结果；每项均有对应不可变 Skill 候选。
`events` 候选通过公开 `streamSkill` 调用，OpenAPI 完整事件 Schema 与 Skill 的 item/output
双重校验；插件只能返回原 HTTP Call 身份，核心才构造 Skill 结果信封。
无正文 Skill 输入为 null，同一生成流键不重发，取消/断线的未知副作用保留原 Call 及物理围栏。
运行 `python3 -m unittest discover -s examples/http -p 'test_http.py' -v` 验证真实 mTLS 方法/状态；
完整 Kernel/OCI 授权、撤权、重启、配额与流测试见 `crates/kernel/tests/http.rs`。
