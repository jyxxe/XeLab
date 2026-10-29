# capabilities 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/capabilities` | [发现当前可调用的能力提供者](listCapabilities.md) |
| GET | `/api/platform/v1/capabilities/{capability_name}/contract` | [读取明确版本的能力契约](getCapabilityContract.md) |
| POST | `/api/platform/v1/calls` | [非流式调用能力](invokeCapability.md) |
| POST | `/api/platform/v1/calls:stream` | [SSE 流式调用能力](streamCapability.md) |
| GET | `/api/platform/v1/calls/{call_id}` | [查询调用结果和结果未知状态](getCall.md) |
| DELETE | `/api/platform/v1/calls/{call_id}` | [取消调用](cancelCall.md) |
