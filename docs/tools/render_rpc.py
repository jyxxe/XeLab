#!/usr/bin/env python3
"""Compile proto descriptors and render one page per RPC. Requires grpcio-tools."""
from pathlib import Path
import json, subprocess, sys, tempfile
from google.protobuf import descriptor_pb2

DOCS=Path(__file__).resolve().parents[1]
PROTO=DOCS/'contracts/proto'
RULES=json.loads((DOCS/'contracts/rpc-semantics.json').read_text())['methods']
files=sorted(PROTO.rglob('*.proto'))
with tempfile.TemporaryDirectory() as tmp:
    descriptor=Path(tmp)/'contract.pb'
    subprocess.run([sys.executable,'-m','grpc_tools.protoc','-I',str(PROTO),'--include_source_info','--include_imports','--descriptor_set_out='+str(descriptor),*[str(f) for f in files]],check=True)
    bundle=descriptor_pb2.FileDescriptorSet.FromString(descriptor.read_bytes())
messages={}
origins={}
for f in bundle.file:
    for message in f.message_type:
        messages['.'+f.package+'.'+message.name]=message
        origins['.'+f.package+'.'+message.name]=f.name
types={1:'double',2:'float',3:'int64',4:'uint64',5:'int32',6:'fixed64',7:'fixed32',8:'bool',9:'string',12:'bytes',13:'uint32',15:'sfixed32',16:'sfixed64',17:'sint32',18:'sint64'}
def table(fullname):
    message=messages[fullname]
    lines=['| 字段号 | 字段 | Proto 类型 | 出现规则 |','|---|---|---|---|']
    for field in message.field:
        label='repeated' if field.label==3 else 'optional' if field.proto3_optional else '按业务校验必填/默认值'
        if field.HasField('oneof_index') and not field.proto3_optional:
            label='oneof '+message.oneof_decl[field.oneof_index].name
        kind=field.type_name.removeprefix('.xelab.v1.') if field.type in [11,14] else types[field.type]
        lines.append(f'| {field.number} | `{field.name}` | `{kind}` | {label} |')
    if not message.field: lines.append('| — | 无字段 | Empty | 无 |')
    return '\n'.join(lines)
index=[]; actual=set()
for f in bundle.file:
    for service in f.service:
        index += [f'## {service.name}','',f'[机器协议源文件](../contracts/proto/{f.name})','']
        for method in service.method:
            key=service.name+'.'+method.name; actual.add(key)
            rule=RULES[key]
            filename=key+'.md'
            index.append(f'- [{key}]({filename})')
            source='../contracts/proto/'+f.name
            inp=method.input_type.rsplit('.',1)[-1]; out=method.output_type.rsplit('.',1)[-1]
            content=[f'# {key}','','[RPC 目录](README.md) · [通用校验](../communication/grpc.md) · [错误](../communication/errors.md)','','> 规格性设计；消息的全部字段及嵌套类型以 Protobuf 为准，字段出现和语义校验两者都必须满足。','',f'`rpc {method.name}({"stream " if method.client_streaming else ""}{inp}) returns ({"stream " if method.server_streaming else ""}{out});`','',f'- 完整方法：`/xelab.v1.{service.name}/{method.name}`',f'- 允许对端：`{rule["peer"]}`；工作负载身份从通道验证，禁止正文自报。',f'- 授权：{rule["permission"]}。',f'- 期限上限：{rule["deadline_ms"]}ms；仍取调用整体 deadline 的更小值。',f'- 幂等：{"方法可幂等处理，但仍须验证当前授权" if rule["idempotent"] else "不保证方法无副作用，按业务契约和稳定幂等键判断，禁止 SDK 默认自动重放"}。','', '## 前置条件、执行及失败语义','',rule['rules'],'','## 请求字段','',table(method.input_type),'','## 响应字段','',table(method.output_type),'',f'嵌套消息、enum、oneof 及字段注释见 [Proto]({source})。公共上下文见 [common.proto](../contracts/proto/xelab/v1/common.proto)。所有字符串标识、期限、摘要和 JSON 大小均执行通用校验；仅在 method 说明允许时可为空。','','## 错误与取消','','校验失败 INVALID_ARGUMENT；无有效身份 UNAUTHENTICATED；越权 PERMISSION_DENIED；契约/状态冲突 FAILED_PRECONDITION；无资源 RESOURCE_EXHAUSTED；期限到期 DEADLINE_EXCEEDED；对端中断 UNAVAILABLE。细节通过 xelab-error-bin 返回，稳定码见错误目录。','','任何有外部副作用的中断不能仅靠 gRPC status 断言未执行；先查询/核实，再按幂等与任务恢复规则处理。']
            path=DOCS/'rpc'/filename;path.parent.mkdir(exist_ok=True);path.write_text('\n'.join(content)+'\n')
        index+=['']
if actual!=set(RULES): raise SystemExit('RPC index mismatch: '+str(actual^set(RULES)))
(DOCS/'rpc/README.md').write_text('# 插件与宿主 RPC\n\n[文档总目录](../README.md) · [Protobuf 契约](../contracts/README.md) · [公共语义](../communication/grpc.md)\n\n'+'\n'.join(index)+'\n')
print('Rendered RPC methods:',len(actual))
