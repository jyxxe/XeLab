"""Validate local Markdown links and the supported GitHub-style anchors offline."""
from html import unescape
from html.parser import HTMLParser
from pathlib import Path
import re
import unicodedata
from urllib.parse import unquote, urlparse


FENCE = re.compile(r'^\s*(`{3,}|~{3,})')
INLINE_CODE = re.compile(r'(?<![\\`]) (`+) (?!`) (.*?) (?<!`) \1 (?!`)', re.X)
LINK = re.compile(r'(?<!!)\[[^\]]*\]\(([^\n)]+)\)')
ATX = re.compile(r'^ {0,3}#{1,6}(?:[ \t]+(.*)|[ \t]*)$')
SETEXT = re.compile(r'^ {0,3}(?:=+|-+)[ \t]*$')
INERT_HTML = re.compile(r'<!--|<(pre|script|style|textarea)\b[^>]*>', re.I)


def _strip_inert_html(line, state):
    """Remove comment/literal HTML sections while respecting inline code spans."""
    spans = [(match.start(), match.end()) for match in INLINE_CODE.finditer(line)]
    result = []
    pos = 0
    while pos < len(line):
        if state:
            end = (re.search(r'-->', line[pos:]) if state == 'comment' else
                   re.search(r'</' + state + r'\s*>', line[pos:], re.I))
            if end is None:
                return ''.join(result), state
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
        pos = match.end()
    return ''.join(result), state


def without_code(value):
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
        line, html_state = _strip_inert_html(line, html_state)
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


def _visible_heading(value):
    """Extract text from the inline forms supported in a heading."""
    code = []

    def save(value):
        code.append(value)
        return f'\x00{len(code)-1}\x00'

    value = INLINE_CODE.sub(lambda match: save(match.group(2)), value)
    value = re.sub(r'\\([!"#$%&\'()*+,\-./:;<=>?@\[\]\\^_`{|}~])',
                   lambda match: save(match.group(1)), value)
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


def _slug(value):
    value = _visible_heading(value).strip().lower()
    return ''.join('-' if c == ' ' else c for c in value
                   if c == ' ' or c in '-_' or unicodedata.category(c)[0] in 'LM'
                   or unicodedata.category(c) == 'Nd')


class _ExplicitAnchors(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.anchors = set()
        self.literal = 0

    def handle_starttag(self, tag, attrs):
        if not self.literal and tag in {'a', 'div', 'span', 'p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6'}:
            self.anchors.update(value for name, value in attrs
                                if value and (name == 'id' or (tag == 'a' and name == 'name')))
        if tag in {'pre', 'code', 'script', 'style', 'textarea'}:
            self.literal += 1

    def handle_endtag(self, tag):
        if tag in {'pre', 'code', 'script', 'style', 'textarea'} and self.literal:
            self.literal -= 1


def markdown_anchors(content):
    """Collect supported heading and explicit anchors; custom anchors do not deduplicate headings."""
    text = without_code(content)
    parser = _ExplicitAnchors()
    parser.feed(INLINE_CODE.sub('', text))
    anchors = {'top'} | parser.anchors
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
        base = _slug(heading)
        candidate = base
        counter = 0
        while candidate in used:
            counter += 1
            candidate = f'{base}-{counter}'
        used.add(candidate)
        if candidate:
            anchors.add(candidate)
    return anchors


def check_markdown_links(file, root, anchor_cache=None):
    """Return local Markdown graph edges and diagnostics for one source file."""
    file = Path(file).resolve()
    root = Path(root).resolve()
    if anchor_cache is None:
        anchor_cache = {}
    content = INLINE_CODE.sub('', without_code(file.read_text(encoding='utf-8')))
    content = re.sub(r'<code\b[^>]*>.*?</code\s*>', '', content, flags=re.I | re.S)
    edges, errors = [], []
    try:
        source = str(file.relative_to(root))
    except ValueError:
        source = str(file)
    for value in LINK.findall(content):
        target = value.strip().strip('<>')
        parsed = urlparse(target)
        if parsed.scheme or parsed.netloc:
            continue
        destination = (file.parent / unquote(parsed.path)).resolve() if parsed.path else file
        if not destination.exists():
            errors.append(f'broken Markdown link {source} -> {target}')
            continue
        if destination.is_file() and destination.suffix.lower() == '.md':
            edges.append(destination)
            if parsed.fragment:
                if destination not in anchor_cache:
                    anchor_cache[destination] = markdown_anchors(destination.read_text(encoding='utf-8'))
                if unquote(parsed.fragment) not in anchor_cache[destination]:
                    errors.append(f'broken Markdown anchor {source} -> {target}')
    return edges, errors
