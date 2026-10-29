#!/usr/bin/env python3
"""从规范 OpenAPI 生成小粒度接口与模型文档；仅使用 Python 标准库。"""
from pathlib import Path
import json
import re

DOCS = Path(__file__).resolve().parents[1]
SPEC = json.loads((DOCS / 'contracts/openapi.json').read_text())
SCHEMAS = SPEC['components']['schemas']
EXAMPLES = {}
UUID = '00000000-0000-4000-8000-000000000001'

def write(path, content):
    target = DOCS / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content.rstrip() + '\n')

def name_of(schema):
    return schema.get('$ref', '').rsplit('/', 1)[-1]

def typename(schema, prefix=''):
    if '$ref' in schema:
        name = name_of(schema)
        return f'[{name}]({prefix}{name}.md)'
    if 'anyOf' in schema:
        return ' / '.join(typename(s, prefix) for s in schema['anyOf'])
    if schema.get('type') == 'array':
        return 'array&lt;' + typename(schema['items'], prefix) + '&gt;'
    kind = schema.get('type', 'JSON')
    return ' / '.join(kind) if isinstance(kind, list) else str(kind)

def limits(schema):
    parts = []
    for key in ['format', 'minimum', 'maximum', 'minLength', 'maxLength', 'minItems', 'maxItems', 'pattern', 'default', 'enum', 'writeOnly','readOnly']:
        if key in schema:
            value = json.dumps(schema[key], ensure_ascii=False)
            parts.append(f'{key}=`{value}`')
    return '; '.join(parts).replace('|', '&#124;') or '见类型定义'

def fields(schema, prefix=''):
    schema = SCHEMAS[name_of(schema)] if '$ref' in schema else schema
    if schema.get('type') != 'object':
        return f'类型：{typename(schema, prefix)}。{schema.get("description", "")}\n'
    lines = ['| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |', '|---|---|---|---|---|']
    for key, value in schema.get('properties', {}).items():
        desc = value.get('description', SCHEMAS.get(name_of(value), {}).get('description', ''))
        lines.append(f'| `{key}` | {typename(value,prefix)} | {"是" if key in schema.get("required",[]) else "否"} | {limits(value)} | {desc.replace("|", "&#124;")} |')
    return '\n'.join(lines) + '\n'

def example(schema, depth=0):
    if '$ref' in schema:
        name = name_of(schema)
        if name == 'InheritanceSpec': return {'mode': 'all_snapshot'}
        if name == 'JsonValue': return {}
        return example(SCHEMAS[name], depth+1)
    if 'example' in schema: return schema['example']
    if 'default' in schema: return schema['default']
    if 'anyOf' in schema:
        if any(s.get('type') == 'null' for s in schema['anyOf']): return None
        return example(schema['anyOf'][0],depth+1)
    if 'enum' in schema: return schema['enum'][0]
    kind = schema.get('type')
    if isinstance(kind,list): return {}
    if kind == 'object':
        required = list(schema.get('required',[]))
        props = dict(schema.get('properties',{}))
        if 'oneOf' in schema:
            choice=schema['oneOf'][0]
            required += choice.get('required',[])
            for key,v in choice.get('properties',{}).items(): props[key]={**props.get(key,{}),**v}
        result={k:example(props[k],depth+1) for k in dict.fromkeys(required) if k in props}
        for key in props:
            if len(result)>=schema.get('minProperties',0): break
            if key not in result: result[key]=example(props[key],depth+1)
        return result
    if 'const' in schema: return schema['const']
    if kind == 'array': return [example(schema['items'],depth+1) for _ in range(schema.get('minItems',0))]
    if kind == 'integer': return schema.get('minimum',0)
    if kind == 'number': return schema.get('minimum',0.0)
    if kind == 'boolean': return False
    if kind == 'null': return None
    if schema.get('format') == 'uuid': return UUID
    if schema.get('format') == 'date-time': return '2026-10-28T12:00:00Z'
    if schema.get('format') == 'uri': return 'https://ui.example.org/callback'
    pattern = schema.get('pattern','')
    if '@sha256:' in pattern: return 'registry.example.org/org/echo@sha256:'+'a'*64
    if pattern.startswith('^sha256:'): return 'sha256:'+'a'*64
    if pattern == '^"rev-[1-9][0-9]*"$': return '"rev-1"'
    if pattern == '^/': return '/data'
    return 'example' if schema.get('maxLength',100)>6 and schema.get('minLength',0)<=7 else 'a'*max(schema.get('minLength',1),1)

def operation_doc(path, method, op):
    domain=op['tags'][0]
    prefix='../../models/'
    auth='本接口定义的公开/登录凭证流程' if not op['security'] else '仅内核可信 Cookie 会话，敏感操作额外校验 CSRF；不接受 API Key' if 'cookieAuth' in op['security'][0] else 'Bearer 会话、用户委托或 API Key；执行前读取当前授权版本'
    lines=[f'# {op["operationId"]} · {op["summary"]}', '', '[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)', '', '> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。', '', f'`{method.upper()} {path}`', '', op['description'], '', '| 项目 | 定义 |','|---|---|',f'| 权限动作 | `{op["x-permission"]}`；同时执行对象范围、完整祖先链及源凭证检查 |', f'| 认证 | {auth} |', '| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |', '| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |', '']
    target=op.get('x-authorization')
    if target:
        lines += [f'授权对象类型：`{target["resource_kind"]}`。目标取值：{target["resource_source"]}。', '',target['conditions'],'']
    if op.get('parameters'):
        lines += ['## 路径、查询及请求头', '', '| 名称 | 位置 | 必填 | 类型与约束 | 含义 |','|---|---|---|---|---|']
        for p in op['parameters']:
            s=p['schema']
            lines.append(f'| `{p["name"]}` | {p["in"]} | {"是" if p.get("required") else "否"} | {typename(s,prefix)}; {limits(s)} | {p.get("description",s.get("description",""))} |')
        lines += ['']
    body=op.get('requestBody')
    if body:
        media,entry=next(iter(body['content'].items()))
        lines += ['## 请求体','',f'Content-Type：`{media}`；请求体必需。','',fields(entry['schema'],prefix)]
        if media in ['application/json','application/x-www-form-urlencoded']:
            value=example(entry['schema'])
            portable_schema=json.loads(json.dumps(entry['schema']).replace('"#/components/schemas/','"../openapi.json#/components/schemas/'))
            EXAMPLES[op['operationId']]={'schema':portable_schema,'value':value}
            lines += ['结构示例（标识和业务数据需替换为当前部署实际获准对象）：','','```json',json.dumps(value,ensure_ascii=False,indent=2),'```','']
    else:
        lines += ['请求体：无。发送非空请求体返回 `400 INVALID_ARGUMENT`。','']
    lines += ['## 响应','','| HTTP | 内容 | 语义 |','|---|---|---|']
    for status,r in op['responses'].items():
        content='；'.join(f'`{m}` → {typename(c["schema"],prefix)}' for m,c in r.get('content',{}).items()) or '无响应体'
        lines.append(f'| {status} | {content} | {r["description"]} |')
    lines += ['']
    if op.get('x-operation-result'):
        lines += [f'异步成功时 `Operation.result` 必须匹配 {typename(op["x-operation-result"],prefix)}；`resource_id` 指向主要结果资源。操作失败时 result=null，error 非空。', '']
    if op.get('x-async-resource') == 'skill_batch':
        lines += ['异步业务批次：`202` 返回 `SkillBatch`；`Location` 指向查询入口，`Retry-After` 给出轮询间隔。持久化后即结束提交连接，进度和结果使用 [batch 协议](../../../skills/batches.md) 查询。','']
    if op.get('x-sensitive-response'):
        if any(p['name']=='Idempotency-Key' for p in op.get('parameters',[])):
            lines += ['一次性响应：签发成功后不存储可重放明文；同键重试返回 `409 ONE_TIME_RESPONSE_UNAVAILABLE` 并可带资源标识。客户端应查询元数据，无法恢复明文时撤销并重新签发。','']
        else:
            lines += ['敏感认证响应禁止缓存或记录原始凭证。授权码仅使用一次；交换失败后按认证流程重新建立授权，不能依靠普通幂等重放取回已签发明文。','']
    if op.get('requestBody') or method in ['post','put','patch','delete']:
        lines += ['## 并发、重试与副作用','']
        if any(p['name']=='Idempotency-Key' for p in op.get('parameters',[])):
            lines += ['`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。','']
        if any(p['name']=='If-Match' for p in op.get('parameters',[])):
            lines += ['目标版本必须与 `If-Match` 一致。检查版本、当前授权、配额及写入在同一事务中完成；不一致返回 412，未提供返回 428。ETag 作用域为本目标资源；特殊子资源使用说明中指定的父资源版本。','']
        lines += ['所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。','']
    write(op['x-document'].replace('operations/','api/operations/',1),'\n'.join(lines))

def main():
    valid_operations={o['operationId'] for methods in SPEC['paths'].values() for o in methods.values()}
    for old in (DOCS/'api/operations').rglob('*.md'):
        if old.name!='README.md' and old.stem not in valid_operations: old.unlink()
    for old in (DOCS/'api/models').glob('*.md'):
        if old.name!='README.md' and old.stem not in SCHEMAS: old.unlink()
    for old in (DOCS/'contracts/schemas').glob('*.schema.json'):
        if old.name.removesuffix('.schema.json') not in SCHEMAS: old.unlink()
    grouped={}
    for path,methods in SPEC['paths'].items():
        for method,op in methods.items():
            operation_doc(path,method,op)
            grouped.setdefault(op['tags'][0],[]).append((method,path,op))
    for domain,items in grouped.items():
        write(f'api/operations/{domain}/README.md',f'# {domain} 接口\n\n[全部 HTTP 接口](../../README.md)\n\n| 方法 | 路径 | 定义 |\n|---|---|---|\n'+'\n'.join(f'| {m.upper()} | `{p}` | [{o["summary"]}]({o["operationId"]}.md) |' for m,p,o in items))
    write('api/README.md','# HTTP 接口目录\n\n[文档总目录](../README.md) · [公共约定](../communication/http.md) · [模型目录](models/README.md) · [OpenAPI](../contracts/openapi.json)\n\n所有字段以 OpenAPI 的显式 Schema 为准，禁止根据示例推断未声明字段。以下按每个操作分文档，每篇包含权限、参数、请求、响应、幂等和并发规则。\n\n'+'\n'.join(f'- [{d}：{len(v)} 个操作](operations/{d}/README.md)' for d,v in grouped.items()))
    model_index=[]
    schema_dir=DOCS/'contracts/schemas'; schema_dir.mkdir(exist_ok=True)
    for name,schema in sorted(SCHEMAS.items()):
        nested=json.loads(json.dumps(schema).replace('"#/components/schemas/','"../openapi.json#/components/schemas/'))
        (schema_dir/(name+'.schema.json')).write_text(json.dumps({'$schema':'https://json-schema.org/draft/2020-12/schema','title':name,**nested},ensure_ascii=False,indent=2)+'\n')
        constraints={k:v for k,v in schema.items() if k not in ['properties','description']}
        lines=[f'# {name}','','[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/'+name+'.schema.json)','','> 状态：规范性设计；由 OpenAPI 生成。','',''+schema.get('description',''),'','## 字段','',fields(schema),'## 对象及跨字段约束','','```json',json.dumps(constraints,ensure_ascii=False,indent=2),'```','','完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。']
        write('api/models/'+name+'.md','\n'.join(lines))
        model_index.append(f'- [{name}]({name}.md)：{schema.get("description", "")}')
    write('api/models/README.md','# 数据模型目录\n\n[HTTP 目录](../README.md) · [机器契约目录](../../contracts/README.md)\n\n'+ '\n'.join(model_index))
    write('contracts/examples/http-requests.json',json.dumps(EXAMPLES,ensure_ascii=False,indent=2))
    print(f'Rendered {sum(map(len,grouped.values()))} operations, {len(SCHEMAS)} models, {len(EXAMPLES)} request examples')

if __name__=='__main__': main()
