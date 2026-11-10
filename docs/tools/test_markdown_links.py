import tempfile
import unittest
from pathlib import Path
from urllib.parse import quote

from markdown_links import check_markdown_links, markdown_anchors


class MarkdownLinksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def write(self, path, content):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content, encoding='utf-8')
        return file

    def test_same_page_cross_page_encoded_and_named_anchors(self):
        other = self.write('other file.md', '''# 中文，标题！
## Repeat
## Repeat-1
## Repeat
<a name="manual-anchor"></a>
<a id="custom-ID"></a>
<span id="span-anchor"></span>
''')
        source = self.write('source.md', '''# My Section
[same](#my-section)
[encoded](other%20file.md#%E4%B8%AD%E6%96%87%E6%A0%87%E9%A2%98)
[first](other%20file.md#repeat)
[collision](other%20file.md#repeat-2)
[named](other%20file.md#manual-anchor)
[custom](other%20file.md#custom-ID)
[html](other%20file.md#span-anchor)
[top](#top)
''')
        edges, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, [])
        self.assertIn(source.resolve(), edges)
        self.assertIn(other.resolve(), edges)

    def test_missing_anchors_and_missing_files_include_source_and_target(self):
        self.write('other.md', '# Present\n')
        source = self.write('source.md', '''# Present
[local](#absent)
[cross](other.md#absent)
[case-sensitive](other.md#Present)
[missing](missing.md#absent)
''')
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, [
            'broken Markdown anchor source.md -> #absent',
            'broken Markdown anchor source.md -> other.md#absent',
            'broken Markdown anchor source.md -> other.md#Present',
            'broken Markdown link source.md -> missing.md#absent',
        ])

    def test_heading_format_punctuation_unicode_and_duplicate_collision(self):
        anchors = markdown_anchors('''# This'll be a _Helpful_ Section About Θ!
## 中文，标点！
## Café &amp; `code.value`
## [Linked text](https://example.com)
## foo_bar
## \\_literal_
Setext title
============
## Repeat
## Repeat-1
## Repeat
## Repeat
## !!!
## !!!
''')
        self.assertTrue({
            'thisll-be-a-helpful-section-about-θ', '中文标点', 'café--codevalue',
            'linked-text', 'foo_bar', '_literal_', 'setext-title', 'repeat', 'repeat-1',
            'repeat-2', 'repeat-3', '-1',
        }.issubset(anchors), anchors)
        self.assertNotIn('repeat-4', anchors)

    def test_code_and_html_comments_do_not_create_links_or_anchors(self):
        source = self.write('source.md', '''# Visible
<!-- # Hidden\n<a name="hidden"></a> [bad](#absent) -->
[real](#visible)
`[inline](#absent)`
````markdown
# Fake
[inside](#absent)
<a name="fake-anchor"></a>
```
# Still fake
````
~~~text
# Also fake
[inside](missing.md)
~~~
<pre><a name="pre-anchor"></a></pre>

    [indented code](#absent)
    # Indented fake

<pre>
# HTML fake
[HTML code](#absent)
</pre>
<code>[inline HTML code](#absent)</code>
<!--
```
-->
## After comment
<pre>
~~~
</pre>
## After HTML
`<!--` [after](#after-comment)
```
code
```
    [indented after fence](#absent)
''')
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, [])
        anchors = markdown_anchors(source.read_text(encoding='utf-8'))
        self.assertIn('visible', anchors)
        self.assertIn('after-comment', anchors)
        self.assertIn('after-html', anchors)
        self.assertTrue({'fake', 'still-fake', 'also-fake', 'fake-anchor',
                         'hidden', 'pre-anchor', 'indented-fake', 'html-fake'}.isdisjoint(anchors))

    def test_indented_list_link_is_still_checked(self):
        source = self.write('source.md', '''# Existing
- list item

    [nested link](#absent)
''')
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, ['broken Markdown anchor source.md -> #absent'])

    def test_external_images_and_non_markdown_fragments_remain_out_of_scope(self):
        self.write('schema.json', '{}')
        source = self.write('source.md', '''# Valid
[remote](https://example.com/missing.md#missing)
[remote-network](//example.com/missing.md#missing)
![image](missing.svg#missing)
[json](schema.json#/missing/pointer)
[local](#valid)
''')
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, [])

    def test_explicit_anchor_names_are_case_sensitive_and_do_not_consume_heading_slug(self):
        source = self.write('source.md', '''<a name="repeat"></a>
## Repeat
## Repeat
[heading](#repeat-1)
[named](#repeat)
[wrong-case](#REPEAT)
''')
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, ['broken Markdown anchor source.md -> #REPEAT'])

    def test_percent_encoded_fragment_is_decoded_once(self):
        self.write('other.md', '# 中文 标题\n<a name="literal%20value"></a>\n')
        source = self.write('source.md', '\n'.join([
            f'[unicode](other.md#{quote("中文-标题")})',
            '[literal-percent](other.md#literal%2520value)',
            '[decoded](other.md#literal%20value)',
        ]))
        _, errors = check_markdown_links(source, self.root)
        self.assertEqual(errors, ['broken Markdown anchor source.md -> other.md#literal%20value'])


if __name__ == '__main__':
    unittest.main()
