#!/usr/bin/env python3
"""Render draft plugin indexes and field pages; never publish runtime artifacts."""
from pathlib import Path
import json

DOCS = Path(__file__).resolve().parents[1]
PRESETS = DOCS / 'preset-plugins'


def read(path):
    return json.loads(path.read_text())


def write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content.rstrip() + '\n')


def literal(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).replace('|', '&#124;')


def schema_type(schema):
    if '$ref' in schema:
        return '`' + schema['$ref'].split('/')[-1] + '`（引用）'
    if 'const' in schema:
        return '`' + literal(schema['const']) + '`'
    if 'enum' in schema:
        return ' / '.join('`' + literal(v) + '`' for v in schema['enum'])
    if 'type' in schema:
        value = schema['type']
        return '`' + (' / '.join(value) if isinstance(value, list) else value) + '`'
    for keyword in ['oneOf', 'anyOf']:
        if keyword in schema:
            return ' 或 '.join(schema_type(x) for x in schema[keyword])
    return '受限 JSON'


def constraints(schema):
    labels = {'minimum': '最小值', 'maximum': '最大值', 'minLength': '最短',
              'maxLength': '最长', 'minItems': '最少项', 'maxItems': '最多项',
              'maxProperties': '最多成员', 'format': '格式', 'pattern': '模式',
              'uniqueItems': '元素唯一', 'contentEncoding': '内容编码'}
    parts = [labels[k] + ' `' + literal(schema[k]) + '`' for k in labels if k in schema]
    if schema.get('additionalProperties') is False:
        parts.append('拒绝未知成员')
    description = schema.get('description', '').replace('|', '&#124;').replace('\n', ' ')
    if description:
        parts.append(description)
    return '；'.join(parts) or '—'


def fields(schema, prefix='', nested=False):
    lines = []
    required = schema.get('required', [])
    for key, value in schema.get('properties', {}).items():
        path = prefix + key
        need = ('父级存在时' if nested else '是') if key in required else '否'
        default = '`' + literal(value['default']) + '`' if 'default' in value else '—'
        lines.append(f'| `{path}` | {schema_type(value)} | {need} | {default} | {constraints(value)} |')
        if value.get('type') == 'object':
            lines.extend(fields(value, path + '.', True))
        if value.get('type') == 'array' and isinstance(value.get('items'), dict):
            item = value['items']
            if item.get('properties'):
                lines.extend(fields(item, path + '[].', True))
            else:
                lines.append(f'| `{path}[]` | {schema_type(item)} | 随数组 | — | {constraints(item)} |')
    return lines


def schema_section(title, file):
    schema = read(file)
    lines = ['## ' + title, '', '[' + file.name + '](' + file.name + ')。']
    if '$ref' in schema:
        ref = schema['$ref']
        lines += ['', '完整字段定义引用 [' + ref.split('/')[-1] + '](' + ref + ')。']
    elif schema.get('type') == 'null':
        lines += ['', '不提供流式业务输出；`null` 为禁用流的契约标记，调用不得发送业务数据帧。']
    else:
        lines += ['', '| 字段 | 类型 | 必填 | 缺省值 | 约束 |', '|---|---|---|---|---|']
        lines += fields(schema)
    if any(k in schema for k in ['oneOf', 'anyOf', 'allOf']):
        lines += ['', '条件分支及互斥字段以机器 Schema 的 `oneOf/anyOf/allOf` 为准；缺省值在验证后应用，不能把缺省解释为允许显式 `null`。']
    return lines + ['']


def operation_page(entry):
    folder = DOCS / entry['folder']
    semantics = read(folder / 'semantics.json')
    cap = entry['capability_id']
    lines = [f'# `{cap}` 字段契约草案', '',
             '[插件接口语义](../../interfaces.md) · [规划目录](../../../planned/README.md)', '',
             '> 规格版本：2.8.0；状态：planned；业务契约版本：0.1.0。尚未发布为运行时能力或 Skill。', '',
             f'能力 ID：`{cap}`；操作目录：`{entry["operation"]}`。', '',
             '下表的字符串长度为 Unicode 码点数；整体 JSON 的 UTF-8 字节、深度与业务字段字节上限独立检查。嵌套必填只在父对象存在时生效。', '']
    for title, name in [('输入', 'input.schema.json'), ('输出', 'output.schema.json'), ('业务流', 'stream.schema.json')]:
        lines += schema_section(title, folder / name)
    lines += ['## 执行约束', '', '[完整执行语义](semantics.json)。', '',
              '| 项目 | 定义 |', '|---|---|',
              f'| 副作用分类 | `{semantics["side_effect"]}` |',
              f'| 最大输入 / 输出 | {semantics["maximum_input_bytes"]} / {semantics["maximum_output_bytes"]} 字节 |',
              f'| JSON 深度 | ≤{semantics["maximum_json_depth"]} |',
              '| 批量 | 核心异步 Skill batch；逐项有界调用，无整批事务 |',
              '| 权限 | 核心根据调用主体、当前祖先链及固定目标计算，业务输入不能替换主体 |',
              '| 失败 | 复用核心错误信封；不以成功结果包裹协议失败 |', '',
              '补充语义与跨字段约束：', '']
    lines += ['- ' + note for note in semantics['notes']]
    lines += ['', '落地门槛：' + '、'.join('`' + gate + '`' for gate in semantics['feature_gates']) + '；见[门槛说明](../../../planned/feature-gates.md)。', '',
              '四文件草案摘要：`' + entry['schema_digest'] + '`。引用文件另在[规划契约目录](../../../planned-contracts.json)锁定；发布时必须打包引用闭包并重新计算正式摘要。']
    return '\n'.join(lines)


def index_page(kind, entries):
    label = '普通插件' if kind == 'ordinary' else '微插件'
    lines = [f'# {label}规划目录', '', '[规划总览](README.md) · [落地门槛](feature-gates.md) · [Skill规划](../../skills/planned/README.md)', '',
             '> 规格版本：2.8.0；状态：planned；各业务契约草案为0.1.0。目录名与能力 ID 按需求保留。', '']
    if kind == 'ordinary':
        lines += ['普通插件运行在独立 CPU OCI 容器中，适合持续服务、外部连接、复杂依赖与独立资源配额。资源默认值须在实现测量后确定并写入正式部署模板。', '']
    else:
        lines += ['微插件统一依赖 `contracts/` 与 Micro Plugin Runtime 执行协议；由 MPR 加载，不为每个微插件创建容器。每微部署默认250mCPU/256MiB，总限额由其全部worker共享，联网默认关闭；资源与联网可在停用排空后通过settings接口手动配置。库须静态打包或由版本化运行时库明确提供；未声明的宿主模块不可调用。', '']
    lines += ['| 目录 | 能力 ID | 依赖项 | 用途 |', '|---|---|---|---|']
    for e in entries:
        deps = []
        for d in e['dependencies']:
            if d == 'contracts/':
                deps.append('[共享契约](../contracts/README.md)')
            elif d == 'micro-plugin-runtime/':
                deps.append('[MPR](../micro-plugin-runtime/README.md)')
            else:
                deps.append('`' + d + '`')
        ids = '、'.join('`' + cap + '`' for cap in e['capability_ids'])
        lines.append(f'| [{e["directory"]}/](../{e["directory"]}/README.md) | {ids} | ' + '；'.join(deps) + ' | ' + e['purpose'] + ' |')
    lines += ['', '这些目录不属于当前可安装预设，也没有自动注册的 Skill；[机器目录](../planned-catalog.json)与[契约目录](../planned-contracts.json)分别记录依赖、门槛及字段定义。', '',
              '普通插件资源与 MPR 总资源都从插件池扣除；内核至少4 vCPU / 16 GiB预留、OS/基础设施预算及单节点不超卖规则不变。多项调用使用核心异步 batch，不在脚本内无限循环代替调度。']
    return '\n'.join(lines)


def render():
    catalog = read(PRESETS / 'planned-catalog.json')
    contracts = read(PRESETS / 'planned-contracts.json')['contracts']
    for entry in contracts:
        write(DOCS / entry['folder'] / 'README.md', operation_page(entry))
    for kind, name in [('ordinary', 'ordinary.md'), ('micro', 'micro.md')]:
        entries = [x for x in catalog['entries'] if x['kind'] == kind]
        write(PRESETS / 'planned' / name, index_page(kind, entries))
    lines = ['# 规划能力逐接口索引', '', '[规划总览](README.md) · [共享接口规则](interface-rules.md)', '',
             '> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。共37个能力入口。', '',
             '| 能力 ID | 插件目录 | 逐字段定义 |', '|---|---|---|']
    for entry in contracts:
        folder = '../' + entry['directory']
        lines.append(f'| `{entry["capability_id"]}` | [{entry["directory"]}]({folder}/README.md) | [输入、输出与执行约束]({folder}/contracts/{entry["operation"]}/README.md) |')
    lines += ['', '字段、分支条件与语义共同生效；原始Schema文件和四文件摘要均可从逐字段页面进入。']
    write(PRESETS / 'planned/contracts.md', '\n'.join(lines))


if __name__ == '__main__':
    render()
    print('Rendered 3 planning indexes and 37 draft capability field pages')
