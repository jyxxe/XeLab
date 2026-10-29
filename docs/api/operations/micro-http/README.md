# micro-http 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings` | [列出微部署HTTP服务商配置](listMicroHttpBindings.md) |
| POST | `/api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings` | [绑定微服务商与托管密钥](createMicroHttpBinding.md) |
| GET | `/api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings/{binding_id}` | [读取单个HTTP绑定](getMicroHttpBinding.md) |
| PUT | `/api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings/{binding_id}` | [更新优先级与HTTP绑定](updateMicroHttpBinding.md) |
| DELETE | `/api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings/{binding_id}` | [撤销微HTTP绑定](deleteMicroHttpBinding.md) |
