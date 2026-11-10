"""Validate local Markdown links and the supported GitHub-style anchors offline."""
from html import unescape
from html.parser import HTMLParser
from pathlib import Path
import re
import unicodedata
from urllib.parse import unquote, urlparse


FENCE = re.compile(r'^\s*(`{3,}|~{3,})')
INLINE_CODE = re.compile(r'(?<![\\`]) (?:\\\\)* (`+) (?!`) (.*?) (?<!`) \1 (?!`)', re.X)
HTML_TAG = re.compile(r'''</?[A-Za-z][A-Za-z0-9-]*(?:\s+(?:[^>"']|"[^"]*"|'[^']*')*)?\s*/?>''')
ATX = re.compile(r'^ {0,3}#{1,6}(?:[ \t]+(.*)|[ \t]*)$')
SETEXT = re.compile(r'^ {0,3}(?:=+|-+)[ \t]*$')
INERT_HTML = re.compile(r'<!--|<(pre|script|style|textarea)\b[^>]*>', re.I)


def _strip_inert_html(line, state, preserve_pre=False):
    """Remove comment/literal HTML sections while respecting inline code spans."""
    spans = [(match.start(), match.end()) for match in INLINE_CODE.finditer(line)]
    result = []
    pos = 0
    while pos < len(line):
        if state:
            end = (re.search(r'-->', line[pos:]) if state == 'comment' else
                   re.search(r'</' + state + r'\s*>', line[pos:], re.I))
            if end is None:
                if preserve_pre and state == 'pre':
                    result.append(line[pos:])
                return ''.join(result), state
            if preserve_pre and state == 'pre':
                result.append(line[pos:pos + end.end()])
            pos += end.end()
            state = None
            continue
        match = INERT_HTML.search(line, pos)
        if match is None:
            result.append(line[pos:])
            break
        if any(start <= match.start() < end for start, end in spans):
            result.append(line[pos:match.end()])
            pos = match.end()
            continue
        result.append(line[pos:match.start()])
        state = 'comment' if match.group() == '<!--' else match.group(1).lower()
        if preserve_pre and state == 'pre':
            result.append(match.group())
        pos = match.end()
    return ''.join(result), state


def without_code(value, preserve_pre=False):
    """Blank code blocks and HTML comments without joining surrounding lines."""
    lines = []
    fence = None
    indented_code = None
    list_indent = None
    previous_blank = True
    html_state = None
    for line in value.splitlines():
        if fence is not None:
            match = FENCE.match(line)
            if match and match.group(1)[0] == fence[0] and len(match.group(1)) >= len(fence):
                if not line[match.end():].strip():
                    fence = None
                    previous_blank = True
            lines.append('')
            continue
        expanded = line.expandtabs(4)
        indent = len(expanded) - len(expanded.lstrip(' '))
        if indented_code is not None:
            if not line.strip() or indent >= indented_code:
                lines.append('')
                continue
            indented_code = None
        if not html_state and list_indent is not None and line.strip() and indent < list_indent:
            list_indent = None
        threshold = (list_indent or 0) + 4
        if not html_state and line.strip() and previous_blank and indent >= threshold:
            indented_code = threshold
            lines.append('')
            continue
        was_pre = html_state == 'pre'
        line, html_state = _strip_inert_html(line, html_state, preserve_pre=preserve_pre)
        if preserve_pre and (was_pre or html_state == 'pre'):
            lines.append(line)
            if html_state is None:
                previous_blank = True
            continue
        if not line.strip():
            lines.append('')
            previous_blank = True
            continue
        expanded = line.expandtabs(4)
        match = FENCE.match(line)
        if match:
            fence = match.group(1)
            lines.append('')
        else:
            list_marker = re.match(r'^ {0,3}(?:[-+*]|\d+[.)])[ \t]+', expanded)
            if list_marker:
                list_indent = list_marker.end()
            lines.append(line)
        previous_blank = False
    return '\n'.join(lines)


def _visible_heading(value, references=None):
    """Extract text from the inline forms supported in a heading."""
    code = []

    def save(value):
        code.append(value)
        return f'\x00{len(code)-1}\x00'

    def code_text(match):
        text = match.group(2)
        if text.startswith(' ') and text.endswith(' ') and text.strip(' '):
            text = text[1:-1]
        return save(text)

    value = INLINE_CODE.sub(code_text, value)
    value = re.sub(r'\\([!"#$%&\'()*+,\-./:;<=>?@\[\]\\^_`{|}~])',
                   lambda match: save(match.group(1)), value)
    if references:
        def reference(match):
            label = match.group(2)
            target = (match.group(3) or label) if match.lastindex == 3 else label
            if _reference_label(target) not in references:
                return match.group()
            return '' if match.group(1) else label

        value = re.sub(r'(!?)\[([^\]]+)\]\[([^\]]*)\]', reference, value)
        value = re.sub(r'(!?)\[([^\]]+)\](?![\[(])', reference, value)
    value = re.sub(r'!\[[^\]]*\]\([^)]*\)', '', value)
    value = re.sub(r'\[([^\]]*)\]\([^)]*\)', r'\1', value)
    value = re.sub(r'<((?:https?://|mailto:)[^>]+)>', r'\1', value)
    value = re.sub(r'<[^>]*>', '', value)
    # Underscores inside words are literal in GitHub-flavored Markdown.
    value = re.sub(r'(?<!\w)__(?=\S)(.+?)(?<=\S)__(?!\w)', r'\1', value)
    value = re.sub(r'(?<!\w)_(?=\S)(.+?)(?<=\S)_(?!\w)', r'\1', value)
    value = unescape(value)
    return re.sub(r'\x00(\d+)\x00',
                  lambda m: code[int(m.group(1))] if int(m.group(1)) < len(code) else m.group(), value)


def _slug(value, references=None):
    value = _visible_heading(value.strip(), references).lower()
    # github-slugger 2.0.0 also retains these letter-like Unicode symbols.
    letter_symbols = ((0x24B6, 0x24E9), (0x1F130, 0x1F149),
                      (0x1F150, 0x1F169), (0x1F170, 0x1F189))
    return ''.join('-' if c == ' ' else c for c in value
                   if c in ' -' or unicodedata.category(c)[0] in 'LM'
                   or unicodedata.category(c) in {'Nd', 'Nl', 'Pc'}
                   or any(start <= ord(c) <= end for start, end in letter_symbols))


class _ExplicitAnchors(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.anchors = set()
        self.stack = []

    def handle_starttag(self, tag, attrs):
        table_children = {'tbody', 'td', 'tfoot', 'th', 'thead', 'tr'}
        if tag in {'a', 'b', 'blockquote', 'br', 'code', 'dd', 'del',
                   'details', 'div', 'dl', 'dt', 'em',
                   'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i', 'img', 'ins',
                   'kbd', 'li', 'mark', 'ol', 'p', 'pre', 'q', 'rp', 'rt', 'ruby',
                   's', 'samp', 'section', 'span', 'strike', 'strong', 'sub',
                   'summary', 'sup', 'table', 'tbody', 'td', 'tfoot', 'th', 'thead',
                   'tr', 'tt', 'ul', 'var'} and (tag not in table_children or 'table' in self.stack):
            self.anchors.update(value for name, value in attrs
                                if value and (name == 'id' or (tag == 'a' and name == 'name')))
        if tag not in {'br', 'hr', 'img'}:
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if tag in self.stack:
            self.stack = self.stack[:len(self.stack) - 1 - self.stack[::-1].index(tag)]


def markdown_anchors(content):
    """Collect supported heading and explicit anchors; custom anchors do not deduplicate headings."""
    text = without_code(content)
    parser = _ExplicitAnchors()
    html = without_code(content, preserve_pre=True)
    parts = re.split(r'(<pre\b[^>]*>.*?</pre\s*>)', html, flags=re.I | re.S)
    parser.feed(''.join(part if i % 2 else INLINE_CODE.sub('', part)
                        for i, part in enumerate(parts)))
    anchors = {'top'} | parser.anchors
    references, _ = _reference_definitions(INLINE_CODE.sub('', text))
    used = set()
    lines = text.splitlines()
    for index, line in enumerate(lines):
        match = ATX.match(line)
        if match:
            heading = re.sub(r'[ \t]+#+[ \t]*$', '', match.group(1) or '')
        elif (SETEXT.match(line) and index and lines[index - 1].strip()
              and not ATX.match(lines[index - 1])
              and not SETEXT.match(lines[index - 1])
              and not re.match(r'^ {0,3}(?:>|[-+*] |\d+[.)] |<)', lines[index - 1])):
            heading = lines[index - 1].strip()
        else:
            continue
        base = _slug(heading, references)
        candidate = base
        counter = 0
        while candidate in used:
            counter += 1
            candidate = f'{base}-{counter}'
        used.add(candidate)
        if candidate:
            anchors.add(candidate)
    return anchors


def _is_escaped(text, pos):
    start = pos
    while start and text[start - 1] == '\\':
        start -= 1
    return (pos - start) % 2 == 1


def _unescape_markdown(value):
    value = re.sub(r'\\([!"#$%&\'()*+,\-./:;<=>?@\[\]\\^_`{|}~])', r'\1', value)
    return unescape(value)


def _bracket_end(text, start):
    depth = 1
    for pos in range(start + 1, len(text)):
        if text[pos] in '[]' and not _is_escaped(text, pos):
            depth += 1 if text[pos] == '[' else -1
            if depth == 0:
                return pos
        if text[pos] == '\n':
            return None
    return None


def _destination(text, start):
    """Parse one inline destination and optional title starting at '('."""
    pos = start + 1
    while pos < len(text) and text[pos] in ' \t':
        pos += 1
    if pos >= len(text):
        return None
    if text[pos] == '<':
        begin = pos + 1
        pos = begin
        while pos < len(text) and text[pos] != '\n':
            if text[pos] == '>' and not _is_escaped(text, pos):
                break
            if text[pos] == '<' and not _is_escaped(text, pos):
                return None
            pos += 1
        if pos >= len(text) or text[pos] != '>':
            return None
        raw = text[begin:pos]
        pos += 1
    else:
        begin = pos
        depth = 0
        while pos < len(text):
            char = text[pos]
            if char in ' \t\n':
                break
            if char == '(' and not _is_escaped(text, pos):
                depth += 1
                if depth > 32:
                    return None
            elif char == ')' and not _is_escaped(text, pos):
                if depth == 0:
                    break
                depth -= 1
            elif char in '<>' and not _is_escaped(text, pos):
                return None
            pos += 1
        if depth:
            return None
        raw = text[begin:pos]
    before_space = pos
    while pos < len(text) and text[pos] in ' \t':
        pos += 1
    if pos < len(text) and text[pos] == ')':
        return raw, pos + 1
    if pos == before_space or pos >= len(text) or text[pos] not in "\"'(":
        return None
    close = ')' if text[pos] == '(' else text[pos]
    pos += 1
    while pos < len(text) and text[pos] != '\n':
        if text[pos] == close and not _is_escaped(text, pos):
            pos += 1
            break
        pos += 1
    else:
        return None
    while pos < len(text) and text[pos] in ' \t':
        pos += 1
    if pos >= len(text) or text[pos] != ')':
        return None
    return raw, pos + 1


def _reference_label(value):
    return ' '.join(_unescape_markdown(value).split()).casefold()


def _reference_definitions(text):
    definitions = {}
    remaining = []
    for line in text.splitlines(keepends=True):
        match = re.match(r'^ {0,3}\[', line)
        end = _bracket_end(line, match.end() - 1) if match else None
        if end is not None and end + 1 < len(line) and line[end + 1] == ':':
            label = _reference_label(line[match.end():end])
            value = line[end + 2:].rstrip('\r\n').strip()
            wrapped = '(' + value + ')'
            parsed = _destination(wrapped, 0) if value else None
            if label and end - match.end() <= 999 and parsed and parsed[1] == len(wrapped):
                definitions.setdefault(label, parsed[0])
                remaining.append(''.join('\n' if char == '\n' else ' ' for char in line))
                continue
        remaining.append(line)
    return definitions, ''.join(remaining)


def _bracket_pairs(text, html):
    pairs, stack = {}, []
    span = 0
    for pos, char in enumerate(text):
        while span < len(html) and html[span][1] <= pos:
            span += 1
        if span < len(html) and html[span][0] <= pos < html[span][1]:
            continue
        if char == '\n':
            stack.clear()
        elif char == '[' and not _is_escaped(text, pos):
            stack.append(pos)
        elif char == ']' and stack and not _is_escaped(text, pos):
            pairs[stack.pop()] = pos
    return pairs


def _markdown_links(content):
    """Yield destinations of inline and single-line reference links outside code."""
    content = INLINE_CODE.sub(lambda m: ' ' * len(m.group()), without_code(content))
    references, content = _reference_definitions(content)
    html = [(m.start(), m.end()) for m in HTML_TAG.finditer(content)]
    pairs = _bracket_pairs(content, html)
    pos = 0
    while pos < len(content):
        if pos not in pairs:
            pos += 1
            continue
        end = pairs[pos]
        label = content[pos + 1:end]
        image = pos > 0 and content[pos - 1] == '!' and not _is_escaped(content, pos - 1)
        after = end + 1
        if after < len(content) and content[after] == '(':
            parsed = _destination(content, after)
            if parsed:
                if not image:
                    yield parsed[0]
                pos = parsed[1]
                continue
        if after < len(content) and content[after] == '[':
            ref_end = pairs.get(after)
            if ref_end is not None:
                ref = content[after + 1:ref_end] or label
                key = _reference_label(ref) if len(ref) <= 999 else ''
                if key in references:
                    if not image:
                        yield references[key]
                pos = ref_end + 1
                continue
        key = _reference_label(label) if references and len(label) <= 999 else ''
        if key in references:
            if not image:
                yield references[key]
            pos = after
        else:
            pos += 1


def check_markdown_links(file, root, anchor_cache=None):
    """Return local Markdown graph edges and diagnostics for one source file."""
    file = Path(file).resolve()
    root = Path(root).resolve()
    if anchor_cache is None:
        anchor_cache = {}
    content = file.read_text(encoding='utf-8')
    edges, errors = [], []
    try:
        source = str(file.relative_to(root))
    except ValueError:
        source = str(file)
    for value in _markdown_links(content):
        target = _unescape_markdown(value)
        parsed = urlparse(target)
        if parsed.scheme or parsed.netloc:
            continue
        destination = (file.parent / unquote(parsed.path)).resolve() if parsed.path else file
        if not destination.exists():
            errors.append(f'broken Markdown link {source} -> {value}')
            continue
        if destination.is_file() and destination.suffix.lower() == '.md':
            edges.append(destination)
            if parsed.fragment:
                if destination not in anchor_cache:
                    anchor_cache[destination] = markdown_anchors(destination.read_text(encoding='utf-8'))
                if unquote(parsed.fragment) not in anchor_cache[destination]:
                    errors.append(f'broken Markdown anchor {source} -> {value}')
    return edges, errors
