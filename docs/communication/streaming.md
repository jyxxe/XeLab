# SSE、取消与背压

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 通用能力 SSE

Content-Type: text/event-stream，Cache-Control: no-store；每帧 `event: <kind>`、`id: <call_id>:<sequence>`、`data: <StreamEvent JSON>`，空行分隔。sequence 从 1 递增，accepted 首帧，item 零到多帧，completed/failed/canceled 恰好一个终帧。

```text
event: accepted
id: 00000000-0000-4000-8000-000000000001:1
data: {"call_id":"00000000-0000-4000-8000-000000000001","sequence":1,"kind":"accepted","payload":{},"terminal":false}

```

accepted 之后插件失败用 failed 终帧，HTTP 已为 200 时不能改成 500。终帧后不得继续 item；连接在无终帧时断开由客户端查询 getCall，不能自动重新生成并拼接结果。

本版生成流不支持 Last-Event-ID 恢复，携带该头返回 400。服务每 15s 可发送 `: ping` 注释保持连接，心跳不计业务 sequence。

## 取消与背压

客户端断线、明确 cancelCall、deadline 到期或授权链失效，均向内核 Broker 及插件 RPC 传播取消。内核取消后拒绝后续子调用，插件在合理边界检查取消；已完成副作用另行核实。

缓冲最多 16 块/4 MiB，单块 1 MiB；不得把整个模型输出先堆在网关内存再发送。慢消费者 30s 不推进则终止，计入调用错误和资源观测。

## 事件订阅 SSE

订阅流 data 为 Delivery，不是 StreamEvent；最多一个未确认 lease，显式 ack 才推进游标。断线不自动 ack，超租约会重投。同 delivery_id 的去重由消费者负责，见 [事件](../tasks/events.md)。

兼容 AI SSE 使用 ChatChunk 与 `[DONE]`，不能与通用能力流混用。
