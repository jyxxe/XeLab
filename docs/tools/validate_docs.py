#!/usr/bin/env python3
"""Read-only cross-contract checks. --strict additionally validates schemas/proto/SQL."""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import tomllib
import sqlite3
from validate_planned import validate_planned
from functools import lru_cache
from urllib.parse import urlparse, unquote

sys.dont_write_bytecode=True

DOCS=Path(__file__).resolve().parents[1]
ROOT=DOCS.parent
ERRORS=[]
def check(ok,message):
    if not ok: ERRORS.append(message)
def no_duplicate_pairs(pairs):
    value={}
    for k,v in pairs:
        if k in value: raise ValueError('duplicate JSON key: '+k)
        value[k]=v
    return value
@lru_cache(maxsize=None)
def read(path): return json.loads(path.read_text(),object_pairs_hook=no_duplicate_pairs)
def walk(value):
    if isinstance(value,dict):
        yield value
        for item in value.values(): yield from walk(item)
    elif isinstance(value,list):
        for item in value: yield from walk(item)
def pointer(data,fragment):
    for part in fragment.removeprefix('/').split('/') if fragment else []:
        data=data[part.replace('~1','/').replace('~0','~')]
    return data
def without_code(value):
    lines=[]; fenced=False
    for line in value.splitlines():
        if line.strip().startswith(('```','~~~')): fenced=not fenced; continue
        if not fenced: lines.append(line)
    return '\n'.join(lines)

def baseline():
    spec=read(DOCS/'contracts/openapi.json')
    schemas=spec['components']['schemas']; operations={}
    for path,methods in spec['paths'].items():
        for method,op in methods.items():
            name=op['operationId']
            check(name not in operations,'duplicate operationId '+name)
            operations[name]=(path,method,op)
            dest=DOCS/'api'/op['x-document']
            check(dest.is_file(),'missing operation doc '+str(dest))
            if dest.exists():check(f'`{method.upper()} {path}`' in dest.read_text(),'wrong route in '+str(dest))
            required=set(re.findall(r'\{([^}]+)\}',path))
            actual={p['name'] for p in op.get('parameters',[]) if p['in']=='path' and p.get('required')}
            check(required==actual,'path params differ: '+name)
            check('x-authorization' in op,'authorization target missing: '+name)
            if op.get('x-authorization'):
                check(op['x-authorization']['resource_kind'] in schemas['ResourceSelector']['properties']['kind']['enum'],'unknown resource kind: '+name)
            if op.get('x-asynchronous'):
                check('202' in op['responses'],'async op must use 202: '+name)
                if op.get('x-async-resource') == 'skill_batch':
                    response=op['responses'].get('202',{})
                    check(response.get('content',{}).get('application/json',{}).get('schema',{}).get('$ref')=='#/components/schemas/SkillBatch','async batch response mismatch: '+name)
                    check('Location' in response.get('headers',{}),'async batch Location missing: '+name)
                else:
                    check('x-operation-result' in op,'async result schema missing: '+name)
    # Resolve every local schema reference without allowing a network fetch.
    json_files=list(DOCS.rglob('*.json'))
    for file in json_files:
        try:data=read(file)
        except Exception as e:ERRORS.append(str(file)+': '+str(e));continue
        for node in walk(data):
            if '$ref' not in node:continue
            value=node['$ref']; parsed=urlparse(value)
            if parsed.scheme in ['http','https','urn']:continue
            try:
                target=(file.parent/unquote(parsed.path)).resolve() if parsed.path else file
                pointer(read(target),unquote(parsed.fragment))
            except Exception as e:ERRORS.append(f'broken $ref in {file.relative_to(DOCS)}: {value}: {e}')
    # Standalone projections must not drift from canonical OpenAPI components.
    for name,schema in schemas.items():
        path=DOCS/'contracts/schemas'/f'{name}.schema.json'
        check(path.exists(),'missing schema projection '+name)
        if path.exists():
            expected=json.loads(json.dumps(schema).replace('"#/components/schemas/','"../openapi.json#/components/schemas/'))
            actual=dict(read(path));actual.pop('$schema',None);actual.pop('title',None)
            check(actual==expected,'schema projection drift '+name)
        check((DOCS/'api/models'/f'{name}.md').exists(),'missing model page '+name)
    # Render operation text in memory: catches stale required fields/headers/statuses.
    module_spec=importlib.util.spec_from_file_location('reference_renderer',DOCS/'tools/render_reference.py')
    renderer=importlib.util.module_from_spec(module_spec);module_spec.loader.exec_module(renderer)
    def compare(path,content):
        target=DOCS/path
        check(target.exists() and target.read_text()==content.rstrip()+'\n','generated operation drift: '+path)
    renderer.write=compare
    for path,method,op in operations.values(): renderer.operation_doc(path,method,op)
    for name,schema in schemas.items():
        p=DOCS/'api/models'/f'{name}.md'
        if p.exists():check(renderer.fields(schema) in p.read_text(),'generated model field table drift: '+name)
    permission=read(DOCS/'contracts/permissions.json')['definitions']
    exempt={'self','public','password','activation_token','authorization_code_or_refresh_token','trusted_login_and_consent'}
    for name,(_,_,op) in operations.items():
        action=op['x-permission']
        if action not in exempt:
            check(action in permission,'unregistered permission '+action)
            if action in permission:
                check(name in permission[action]['operations'],'operation absent in permission catalog: '+name)
                check(op['x-authorization']['resource_kind'] in permission[action]['resource_kinds'],'permission target mismatch: '+name)
    internal=read(DOCS/'contracts/internal-interfaces.json')['methods']
    definitions=read(DOCS/'contracts/internal.schema.json')['$defs']
    for m in internal:
        for field in ['context','request','response']:
            check(m[field] in definitions or m[field] in schemas,'unknown internal type '+m[field])
        check((DOCS/'internal'/f'{m["module"]}.md').is_file(),'missing internal module page '+m['module'])
    # Markdown links, page size and reachability.
    md=list(DOCS.rglob('*.md'))+[ROOT/'README.md',ROOT/'plan/v0.md']
    graph={}
    for file in md:
        content=file.read_text()
        if 'archive' not in file.parts:
            check(len(content.splitlines())<=220,'human page exceeds 220 lines: '+str(file.relative_to(ROOT)))
        edges=[]
        for value in re.findall(r'(?<!!)\[[^\]]*\]\(([^\n)]+)\)',without_code(content)):
            target=value.strip().strip('<>');parsed=urlparse(target)
            if parsed.scheme or target.startswith('#'):continue
            destination=(file.parent/unquote(parsed.path)).resolve()
            check(destination.exists(),f'broken Markdown link {file.relative_to(ROOT)} -> {target}')
            if destination.is_file() and destination.suffix=='.md':edges.append(destination)
        graph[file.resolve()]=edges
    seen=set();pending=[(DOCS/'README.md').resolve()]
    while pending:
        p=pending.pop()
        if p in seen:continue
        seen.add(p);pending.extend(graph.get(p,[]))
    for p in DOCS.rglob('*.md'):check(p.resolve() in seen,'unreachable Markdown page: '+str(p.relative_to(DOCS)))
    provenance=read(DOCS/'contracts/provenance.json')
    archive=DOCS/provenance['archive']
    check(hashlib.sha256(archive.read_bytes()).hexdigest()==provenance['sha256'],'original plan archive checksum changed')
    check(len(read(DOCS/'contracts/examples/http-requests.json'))==sum('requestBody' in op and any(m in op['requestBody']['content'] for m in ['application/json','application/x-www-form-urlencoded']) for _,_,op in operations.values()),'request examples coverage mismatch')
    return spec,operations,internal,len(md)-2

def strict(spec,operations):
    try:
        from openapi_spec_validator import validate_spec
        from jsonschema import Draft202012Validator, FormatChecker
        from referencing import Registry, Resource
        from referencing.jsonschema import DRAFT202012
        from google.protobuf import descriptor_pb2
        from pglast import parse_sql
    except ImportError as e:
        ERRORS.append('strict dependencies missing; install docs/tools/requirements.txt: '+str(e));return
    try:validate_spec(spec)
    except Exception as e:ERRORS.append('OpenAPI invalid: '+str(e))
    for name,schema in spec['components']['schemas'].items():
        try:Draft202012Validator.check_schema(schema)
        except Exception as e:ERRORS.append('invalid component Schema '+name+': '+str(e))
    def retrieve(uri):
        parsed=urlparse(uri)
        if parsed.scheme!='file':raise ValueError('network schema retrieval forbidden: '+uri)
        path=Path(unquote(parsed.path)).resolve()
        if not path.is_relative_to(DOCS):raise ValueError('schema outside docs: '+uri)
        return Resource.from_contents(read(path),default_specification=DRAFT202012)
    registry=Registry(retrieve=retrieve)
    checker=FormatChecker()
    def validate_case(schema,value,label):
        try:Draft202012Validator(schema,registry=registry,format_checker=checker).validate(value)
        except Exception as e:ERRORS.append('example/schema validation failed '+label+': '+str(e))
    base=(DOCS/'contracts/openapi.json').as_uri()
    for name,item in read(DOCS/'contracts/examples/http-requests.json').items():
        ref=item['schema'].get('$ref')
        if ref and ref.startswith('#'):schema={'$ref':base+ref}
        elif ref:
            uri,fragment=ref.split('#')
            schema={'$ref':(DOCS/'contracts/examples'/uri).resolve().as_uri()+'#'+fragment}
        else:schema=item['schema']
        validate_case(schema,item['value'],name)
    for item in read(DOCS/'contracts/examples/index.json'):
        ref=item['schema']['$ref'];uri,fragment=ref.split('#')
        absolute=(DOCS/'contracts/examples'/uri).resolve().as_uri()+'#'+fragment
        validate_case({'$ref':absolute},read(DOCS/'contracts/examples'/item['file']),item['file'])
    for name in ['internal.schema.json','kernel-config.schema.json','ui-bridge.schema.json']:
        try:Draft202012Validator.check_schema(read(DOCS/'contracts'/name))
        except Exception as e:ERRORS.append('invalid schema '+name+': '+str(e))
    # Boundary fixtures check both accepted and rejected protocol structures.
    for item in read(DOCS/'contracts/examples/ui-batch-validation.json'):
        uri,fragment=item['schema']['$ref'].split('#')
        absolute=(DOCS/'contracts/examples'/uri).resolve().as_uri()+'#'+fragment
        try:
            accepted=Draft202012Validator({'$ref':absolute},registry=registry,format_checker=checker).is_valid(item['value'])
            check(accepted==item['valid'],'UI/batch boundary fixture mismatch: '+item['name'])
        except Exception as e:ERRORS.append('UI/batch fixture failed '+item['name']+': '+str(e))
    validate_case({'$ref':(DOCS/'contracts/kernel-config.schema.json').as_uri()},tomllib.loads((DOCS/'contracts/examples/kernel.toml').read_text()),'kernel.toml')
    proto=DOCS/'contracts/proto'
    with tempfile.TemporaryDirectory() as temp:
        descriptor=Path(temp)/'contract.pb'
        cmd=[sys.executable,'-m','grpc_tools.protoc','-I',str(proto),'--include_imports','--descriptor_set_out='+str(descriptor),*[str(p) for p in sorted(proto.rglob('*.proto'))]]
        result=subprocess.run(cmd,capture_output=True,text=True)
        check(result.returncode==0,'Proto compile failed: '+result.stderr)
        if descriptor.exists():
            data=descriptor_pb2.FileDescriptorSet.FromString(descriptor.read_bytes())
            methods={s.name+'.'+m.name for f in data.file for s in f.service for m in s.method}
            rules=read(DOCS/'contracts/rpc-semantics.json')['methods']
            check(methods==set(rules),'RPC source/semantics mismatch')
            for name in methods:check((DOCS/'rpc'/f'{name}.md').exists(),'missing RPC page '+name)
    try:parse_sql((DOCS/'contracts/metadata.sql').read_text())
    except Exception as e:ERRORS.append('PostgreSQL SQL syntax invalid: '+str(e))
    # Echo package digest is an actual fixture, not an arbitrary placeholder.
    folder=DOCS/'contracts/examples/echo'
    entries=''.join(name+' '+hashlib.sha256((folder/name).read_bytes()).hexdigest()+'\n' for name in ['input.schema.json','output.schema.json','stream.schema.json','semantics.json'])
    expected='sha256:'+hashlib.sha256(entries.encode()).hexdigest()
    manifest=read(DOCS/'contracts/examples/plugin-manifest.json')
    check(manifest['capabilities'][0]['contract']['schema_digest']==expected,'echo contract digest mismatch')
    for capability in manifest['capabilities']:
        for name in ['input_schema','output_schema']:
            try:Draft202012Validator.check_schema(capability[name])
            except Exception as e:ERRORS.append('invalid example capability schema: '+str(e))
    # Preset contracts and routing declarations must agree across all copies.
    presets=read(DOCS/'preset-plugins/catalog.json')['presets']
    check(len({x['plugin_id'] for x in presets})==len(presets),'duplicate preset plugin ID')
    all_skills={}
    for file in (DOCS/'skills').rglob('*.skill.json'):
        definition=read(file);all_skills[definition['name']]=definition
        validate_case({'$ref':base+'#/components/schemas/SkillDefinition'},definition,str(file.relative_to(DOCS)))
        for key in ['input_schema','output_schema']:
            try:Draft202012Validator.check_schema(definition[key])
            except Exception as e:ERRORS.append('invalid Skill schema '+str(file)+': '+str(e))
    expected_resources={'embedding-gemma-text-8':(1000,4294967296,8),'embedding-gemma-text-1':(2000,4294967296,1),'embedding-gemma-multimodal-1':(1000,6442450944,1)}
    for preset in presets:
        folder=DOCS/'preset-plugins'/preset['directory'];m=read(folder/preset['manifest']);cfg=read(folder/preset['defaults']);deployment=read(folder/preset['deployment_template'])
        validate_case({'$ref':base+'#/components/schemas/Manifest'},m,preset['directory']+' manifest')
        validate_case(m['config_schema'],cfg,preset['directory']+' defaults')
        validate_case({'$ref':base+'#/components/schemas/DeploymentSpec'},deployment,preset['directory']+' deployment')
        check(deployment['enabled'] is False and deployment['replicas']==1,'preset must default disabled/single replica: '+preset['directory'])
        check(deployment['config']==cfg and deployment['limits']==preset['default_limits'],'preset/default/deployment drift '+preset['directory'])
        check(m['concurrency']['max_active_requests']==preset['max_active_requests'],'preset concurrency drift '+preset['directory'])
        if preset['directory'] in expected_resources:
            cpu,mem,concurrency=expected_resources[preset['directory']]
            check((deployment['limits']['cpu_millis'],deployment['limits']['memory_bytes'],m['concurrency']['max_active_requests'])==(cpu,mem,concurrency),'user-required embedding resources changed '+preset['directory'])
        caps={x['contract']['name']:x for x in m['capabilities']}
        for definition in m.get('skills',[]):
            check(all_skills.get(definition['name'])==definition,'manifest/Skill copy mismatch '+definition['name'])
            check(definition['target']['plugin_id']==m['plugin_id'],'Skill target plugin mismatch '+definition['name'])
            name=definition['target']['contract']['name'];cap=caps.get(name)
            check(cap is not None,'Skill target not declared '+name)
            if cap:
                check(cap['contract']==definition['target']['contract'],'Skill contract digest mismatch '+name)
                check(cap['input_schema']==definition['input_schema'] and cap['output_schema']==definition['output_schema'],'Skill request/response mismatch '+name)
    for entry in read(DOCS/'preset-plugins/contracts/registry.json')['contracts']:
        folder=(DOCS/'preset-plugins/contracts'/entry['folder']).resolve()
        parts=['input.schema.json','output.schema.json','stream.schema.json','semantics.json']
        data=''.join(name+' '+hashlib.sha256((folder/name).read_bytes()).hexdigest()+'\n' for name in parts)
        digest='sha256:'+hashlib.sha256(data.encode()).hexdigest()
        check(digest==entry['digest'],'preset raw contract digest mismatch '+entry['name'])
    for entry in read(DOCS/'skills/examples/index.json'):
        folder=DOCS/'skills/examples';value=read(folder/entry['file']);definition=read((folder/entry['schema_file']).resolve())
        validate_case({'$ref':base+'#/components/schemas/SkillInvokeRequest'},value,'Skill envelope '+entry['file'])
        validate_case(definition['input_schema'],value['input'],'Skill business input '+entry['file'])
    # Knowledge queries return complete ID lists; obsolete paging fields are rejected.
    kb_folder=DOCS/'preset-plugins/knowledge-base'
    for item in read(kb_folder/'contracts/query-validation.json'):
        schema={'$ref':(kb_folder/'contracts'/item['schema']).as_uri()}
        try:
            accepted=Draft202012Validator(schema,registry=registry,format_checker=checker).is_valid(item['value'])
            check(accepted==item['valid'],'knowledge query boundary fixture mismatch: '+item['name'])
        except Exception as e:ERRORS.append('knowledge query fixture failed '+item['name']+': '+str(e))
    cache_schema=read(kb_folder/'manifest.json')['config_schema']['properties']['query_cache']
    for item in read(kb_folder/'contracts/cache-validation.json'):
        accepted=Draft202012Validator(cache_schema).is_valid(item['value'])
        if accepted:
            total=sum(item['value'][n] for n in ['data_max_bytes','text_max_bytes','vector_max_bytes'])
            accepted=total<=2147483648 and total<=item.get('memory_bytes',4294967296)//2
        check(accepted==item['valid'],'knowledge cache boundary fixture mismatch: '+item['name'])
    query_result=read(DOCS/'skills/examples/kb-query-result.json')
    check(query_result['count']==len(query_result['ids']),'complete query result count mismatch')
    stages=query_result['vector_stages']
    check([s['stage_index'] for s in stages]==sorted({s['stage_index'] for s in stages}),'vector stages must be ordered/unique')
    for stage in stages:
        check(sum(stage[n] for n in ['indexed_rows','pending_rows','failed_rows','skipped_rows'])==stage['input_count'],'query vector candidate counts mismatch')
        check(stage['output_count']<=min(stage['k'],stage['indexed_rows']),'query topk output exceeds valid candidates')
    try:
        Draft202012Validator.check_schema(read(DOCS/'preset-plugins/model-lock.schema.json'))
        Draft202012Validator.check_schema(read(DOCS/'preset-plugins/knowledge-base/types.schema.json'))
        connection=sqlite3.connect(':memory:')
        connection.executescript((DOCS/'preset-plugins/knowledge-base/storage.sql').read_text())
        dims=next(c[4] for c in connection.execute('PRAGMA table_info(kb_vector_indexes)') if c[1]=='dimensions')
        check(str(dims)=='128','knowledge vector storage must default to 128 dimensions')
        check(connection.execute("SELECT value FROM kb_metadata WHERE key='data_schema_version'").fetchone()[0]=='2','knowledge data schema version must be2')
        # These exercise the SQL cache watermark mechanism, not an unimplemented DSL engine.
        uid='00000000-0000-4000-8000-000000000001'
        connection.execute('INSERT INTO kb_tables(id,owner_id,name,schema_revision,data_revision,created_at) VALUES(?,?,?,?,?,?)',(uid,uid,'sample',1,1,'2026-10-29T00:00:00Z'))
        connection.execute('INSERT INTO kb_fields(id,table_id,name,type_json,nullable,has_default,ordinal) VALUES(?,?,?,?,?,?,?)',(uid,uid,'content','{"kind":"text"}',1,0,0))
        connection.execute('INSERT INTO kb_rows(table_id,id,revision,cells_json,created_at,updated_at) VALUES(?,?,?,?,?,?)',(uid,uid,1,'{}','2026-10-29T00:00:00Z','2026-10-29T00:00:00Z'))
        connection.execute("INSERT INTO kb_vector_indexes(field_id,state) VALUES(?,'pending')",(uid,))
        revision=lambda:connection.execute('SELECT index_revision FROM kb_vector_indexes WHERE field_id=?',(uid,)).fetchone()[0]
        check(revision()==1,'index revision must begin at1')
        connection.execute("INSERT INTO kb_vectors(table_id,row_id,field_id,generation,source_hash,dimensions,state,updated_at) VALUES(?,?,?,?,?,?,?,?)",(uid,uid,uid,0,'sha256:'+'a'*64,128,'pending','2026-10-29T00:00:00Z'))
        check(revision()==2,'vector insertion must invalidate query cache')
        connection.execute("UPDATE kb_vectors SET state='ready',embedding_space_id=?,value_f32le=? WHERE field_id=?",('sha256:'+'a'*64,bytes.fromhex('0000803f')+bytes(508),uid))
        check(revision()==3,'pending-to-ready must invalidate query cache without generation/data changes')
        connection.execute('SAVEPOINT cache_revision_rollback')
        connection.execute("UPDATE kb_vectors SET state='failed' WHERE field_id=?",(uid,))
        check(revision()==4,'vector state update must increment revision')
        connection.execute('ROLLBACK TO cache_revision_rollback')
        connection.execute('RELEASE cache_revision_rollback')
        check(revision()==3,'vector and cache revision rollback must be atomic')
        connection.execute('DELETE FROM kb_vectors WHERE field_id=?',(uid,))
        check(revision()==4,'vector deletion must invalidate query cache')
        connection.execute('UPDATE kb_vector_indexes SET generation=generation+1 WHERE field_id=?',(uid,))
        check(revision()==5,'generation change must not reset index revision')
        connection.close()
    except Exception as e:ERRORS.append('knowledge SQL/type validation failed: '+str(e))
    kb=read(DOCS/'preset-plugins/knowledge-base/defaults.json')
    check(kb['default_vector_dimensions']==128,'knowledge-base default dimension must be128')
    check('maximum_query_rows' not in kb and kb['maximum_query_ids']==100000,'query configuration must bound complete IDs instead of a result page')
    check(all(kb['query_cache'][n]==128000000 for n in ['data_max_bytes','text_max_bytes','vector_max_bytes']),'all three knowledge cache defaults must be128MB each')
    check(kb['embedding_routing']['text_query_preference'][0]=='org.xelab.embedding-gemma.text-1','query embedding must default to text-1')
    check(kb['embedding_routing']['text_index_preference'][0]=='org.xelab.embedding-gemma.text-8','index embedding must default to text-8')
    check(kb['embedding_routing']['allow_fallback'] is True,'knowledge embedding fallback must default enabled')
    # Micro artifacts have an independent core provenance, not a runtime OCI signature.
    micro=DOCS/'preset-plugins/micro-plugin-runtime'
    for item in read(micro/'examples/validation.json'):
        schema={'$ref':base+'#/components/schemas/'+item['schema']}
        accepted=Draft202012Validator(schema,registry=registry,format_checker=checker).is_valid(item['value'])
        check(accepted==item['valid'],'micro boundary fixture mismatch: '+item['name'])
    artifact=read(micro/'examples/artifact.json');source=(micro/'examples'/artifact['source_file']).read_bytes()
    manifest=read(micro/'examples'/artifact['manifest_file'])
    source_digest='sha256:'+hashlib.sha256(source).hexdigest()
    check(source_digest==artifact['source_digest'],'micro source digest mismatch')
    # This fixture has only integer numbers and ASCII keys; canonical form equals JCS.
    package={'manifest':manifest,'source_digest':source_digest}
    digest='sha256:'+hashlib.sha256(json.dumps(package,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
    check(digest==artifact['package_digest'],'micro package digest mismatch')
    publication=read(micro/'examples/publish-version.json');definition=read(micro/'examples/echo.skill.json')
    check(publication['manifest']==manifest and publication['source_digest']==source_digest,'micro publish fixture drift')
    validate_case({'$ref':base+'#/components/schemas/SkillDefinition'},definition,'generated micro Skill')
    check(definition['target']['package_digest']==digest,'micro target package mismatch')
    check(definition['input_schema']==manifest['input_schema'] and definition['output_schema']==manifest['output_schema'],'micro business schema drift')
    request=read(micro/'examples/invoke.json')
    validate_case(manifest['input_schema'],request['input'],'micro sample input')
    validate_case(manifest['output_schema'],read(micro/'examples/output.json'),'micro sample output')
    check(0<len(source)<=262144 and not source.startswith(b'\xef\xbb\xbf') and b'\x00' not in source,'invalid micro source fixture encoding/size')
    source.decode('utf-8',errors='strict')
    runtime=read(micro/'manifest.json');cfg=read(micro/'defaults.json');deployment=read(micro/'deployment.template.json')
    check(runtime.get('micro_runtime',{}).get('api_major')==1,'MPR descriptor missing')
    check(not runtime['skills'] and not runtime['capabilities'],'MPR must not expose a generic execute Skill/capability bypass')
    check(runtime['requested_rules']==[{'rule':{'action':'micro.http.request','resource':{'kind':'micro_plugin','match':'all'}},'required':False}] and runtime['volumes']==[],'MPR must only request optional brokered HTTP, never raw Secret/volume permissions')
    check(cfg['artifact_cache_max_bytes']+cfg['maximum_sandbox_bytes']+cfg['control_memory_reserve_bytes']<=deployment['limits']['memory_bytes'],'MPR memory budget exceeds parent cgroup limit')
    check(cfg['maximum_active_requests']*cfg['worker_address_space_bytes']<=cfg['maximum_sandbox_bytes'],'MPR worker admission exceeds sandbox budget')
    # Micro presets are separate source artifacts, not OCI install/volume templates.
    for preset in read(DOCS/'preset-plugins/catalog.json').get('micro_presets',[]):
        folder=DOCS/'preset-plugins'/preset['directory'];manifest=read(folder/preset['manifest'])
        validate_case({'$ref':base+'#/components/schemas/MicroManifest'},manifest,preset['directory']+' MicroManifest')
        validate_case({'$ref':base+'#/components/schemas/CreateMicroDeployment'},read(folder/preset['deployment_template']),preset['directory']+' micro deployment')
        validate_case(read(folder/preset['defaults_schema']),read(folder/preset['defaults']),preset['directory']+' defaults')
        artifact=read(folder/preset['artifact']);source=(folder/artifact['source_file']).read_bytes()
        sha='sha256:'+hashlib.sha256(source).hexdigest()
        check(sha==artifact['source_digest'],'micro preset source digest mismatch '+preset['directory'])
        digest='sha256:'+hashlib.sha256(json.dumps({'manifest':manifest,'source_digest':sha},sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
        check(digest==artifact['package_digest'],'micro preset package digest mismatch '+preset['directory'])
        definition=read(folder/'search.skill.example.json')
        validate_case({'$ref':base+'#/components/schemas/SkillDefinition'},definition,preset['directory']+' generated candidate')
        check(definition['target']['package_digest']==digest and definition['input_schema']==manifest['input_schema'] and definition['output_schema']==manifest['output_schema'],'micro preset candidate/source mismatch '+preset['directory'])
        check(manifest['side_effect']=='non_repeatable' and manifest['execution_profile']=='controlled-http-v1','search billing profile must forbid automatic replay')
        for binding in read(folder/'http-bindings.example.json'):
            validate_case({'$ref':base+'#/components/schemas/MicroHttpBindingSpec'},binding,'search binding '+binding['endpoint_alias'])
        for item in read(folder/'validation.json'):
            accepted=Draft202012Validator({'$ref':base+'#/components/schemas/'+item['schema']},registry=registry,format_checker=checker).is_valid(item['value'])
            check(accepted==item['valid'],'search HTTP fixture mismatch '+item['name'])
        validate_case(manifest['input_schema'],read(folder/'invoke.example.json')['input'],'search business input')
    object_folder=DOCS/'preset-plugins/object-storage'
    for item in read(object_folder/'validation.json'):
        schema={'$ref':(object_folder/'contracts'/item['contract']/'input.schema.json').as_uri()}
        accepted=Draft202012Validator(schema,registry=registry,format_checker=checker).is_valid(item['value'])
        check(accepted==item['valid'],'object-storage fixture mismatch '+item['name'])
    # Exercise durable namespace and anti-ABA constraints without claiming a file service exists.
    try:
        connection=sqlite3.connect(':memory:');connection.executescript((object_folder/'storage.sql').read_text())
        connection.execute('INSERT INTO os_users(owner_id,quota_bytes,max_objects) VALUES(?,?,?)',('user-a',1073741824,10000))
        connection.execute('INSERT INTO os_users(owner_id,quota_bytes,max_objects) VALUES(?,?,?)',('user-b',1073741824,10000))
        for owner,cid in [('user-a','content-a'),('user-b','content-b')]:
            connection.execute("INSERT INTO os_contents(id,owner_id,private_file_key,size_bytes,digest,state,created_at) VALUES(?,?,?,?,?,'committed',?)",(cid,owner,cid,0,'sha256:'+hashlib.sha256(b'').hexdigest(),'2026-10-29T00:00:00Z'))
            connection.execute("INSERT INTO os_heads(owner_id,logical_key,revision,state,object_id,content_id,content_type,created_at,updated_at) VALUES(?,?,1,'live',?,?,?,? ,?)",(owner,'same.txt','object-'+owner,cid,'text/plain','2026-10-29T00:00:00Z','2026-10-29T00:00:00Z'))
        check(connection.execute('SELECT count(*) FROM os_heads WHERE logical_key=?',('same.txt',)).fetchone()[0]==2,'object namespaces must support same key per user')
        connection.execute("UPDATE os_heads SET revision=2,state='deleted',object_id=NULL,content_id=NULL WHERE owner_id='user-a'")
        connection.execute("UPDATE os_heads SET revision=3,state='live',object_id='new-object-a',content_id='content-a' WHERE owner_id='user-a'")
        stale=connection.execute("UPDATE os_heads SET revision=4,state='deleted',object_id=NULL,content_id=NULL WHERE owner_id='user-a' AND revision=1")
        check(stale.rowcount==0,'stale pre-delete revision must not delete recreated object')
        for statement in ["UPDATE os_heads SET revision=1 WHERE owner_id='user-a'", "DELETE FROM os_heads WHERE owner_id='user-a'", "UPDATE os_heads SET revision=4,content_id='content-b' WHERE owner_id='user-a'"]:
            try:connection.execute(statement)
            except sqlite3.IntegrityError:pass
            else:check(False,'object invariant accepted illegal mutation: '+statement)
        check(connection.execute("SELECT value FROM os_metadata WHERE key='data_schema_version'").fetchone()[0]=='1','object storage schema version drift')
        connection.close()
    except Exception as e:ERRORS.append('object storage SQL/invariant validation failed: '+str(e))

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--strict',action='store_true');args=parser.parse_args()
    spec,operations,internal,pages=baseline()
    planned=validate_planned(DOCS,check,read,strict=args.strict)
    import render_planned
    def compare_planned(path,content):
        check(path.exists() and path.read_text()==content.rstrip()+'\n','generated planned page drift: '+str(path.relative_to(DOCS)))
    render_planned.write=compare_planned
    render_planned.render()
    if args.strict:strict(spec,operations)
    if ERRORS:
        print('\n'.join('ERROR '+e for e in ERRORS));print(f'FAILED: {len(ERRORS)} issues');return 1
    rpc=len(read(DOCS/'contracts/rpc-semantics.json')['methods'])
    print(f'PASS: {pages} Markdown pages; {len(operations)} HTTP operations; {len(spec["components"]["schemas"])} models; {rpc} RPCs; {len(internal)} internal methods; {planned["directories"]} planned plugins / {planned["capabilities"]} draft capabilities / {planned["fixtures"]} fixtures; strict={args.strict}')
    return 0
if __name__=='__main__':sys.exit(main())
