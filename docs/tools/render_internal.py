#!/usr/bin/env python3
"""Render in-process interface pages from the checked-in type/method contracts."""
from pathlib import Path
import json
D=Path(__file__).resolve().parents[1]
metadata=json.loads((D/'contracts/internal-interfaces.json').read_text())
types=json.loads((D/'contracts/internal.schema.json').read_text())['$defs']
http=json.loads((D/'contracts/openapi.json').read_text())['components']['schemas']
def typename(s):
    if '$ref' in s:return s['$ref'].rsplit('/',1)[-1]
    if 'anyOf' in s:return ' / '.join(typename(x) for x in s['anyOf'])
    return str(s.get('type',s.get('enum','JSON')))
grouped={}
for method in metadata['methods']:grouped.setdefault(method['module'],[]).append(method)
for module,methods in grouped.items():
    lines=[f'# {module} 模块接口','','[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)','','> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。','']
    for m in methods:
        lines += [f'## {module}.{m["method"]}','',f'`async fn {m["method"]}(ctx: &AuthContext, request: {m["request"]}) -> Result<{m["response"]}, PlatformError>`','',f'期限上限：{m["deadline_ms"]}ms。事务：{m["transaction"]}。','',m['rule'],'','| 输入字段 | 类型 | 语义 |','|---|---|---|']
        schema=types.get(m['request'],http.get(m['request']))
        for key,value in schema.get('properties',{}).items():lines.append(f'| `{key}` | `{typename(value)}` | {value.get("description","见引用类型的完整约束").replace("|"," / ")} |')
        lines += ['',f'输出 `{m["response"]}` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。','','错误：'+', '.join('`'+c+'`' for c in m['errors'])+'。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。','']
    (D/'internal'/f'{module}.md').write_text('\n'.join(lines)+'\n')
(D/'internal/README.md').write_text('# 内核进程内接口\n\n[文档总目录](../README.md) · [结构契约](../contracts/internal.schema.json) · [方法清单](../contracts/internal-interfaces.json)\n\nHTTP handler 复用对应 operationId 的结构；下列为稳定模块协作边界，私有辅助函数不纳入稳定接口。\n\n'+'\n'.join(f'- [{module}]({module}.md)：{len(methods)}个方法' for module,methods in grouped.items())+'\n')
print('Rendered internal methods:',sum(map(len,grouped.values())))
