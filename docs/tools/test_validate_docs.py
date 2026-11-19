import tempfile
from pathlib import Path
import unittest

from validate_docs import markdown_anchors, markdown_link_targets, without_code


class MarkdownAnchorTests(unittest.TestCase):
    def test_github_headings_and_custom_anchors(self):
        with tempfile.TemporaryDirectory() as directory:
            document = Path(directory) / 'example.md'
            document.write_text(
                '# HostService\n'
                '## 微部署资源与联网设置\n'
                '## A [Link](other.md) &amp; `rpc_method`! ###\n'
                '## Literal `[label](destination.md)`\n'
                '<a name="CustomAnchor"></a>\n'
                '<a id="hostservice-1"></a>\n'
                '### HostService\n'
                '### HostService-1\n'
                '~~~markdown\n'
                '## Hidden Header\n'
                '<a id="HiddenAnchor"></a>\n'
                '~~~\n',
                encoding='utf-8',
            )
            self.assertEqual(markdown_anchors(document), {
                'hostservice', '微部署资源与联网设置', 'a-link--rpc_method', 'literal-labeldestinationmd',
                'CustomAnchor', 'hostservice-1', 'hostservice-1-1',
            })

    def test_nested_markdown_example_does_not_define_anchors(self):
        with tempfile.TemporaryDirectory() as directory:
            document = Path(directory) / 'example.md'
            document.write_text(
                '## Before\n'
                '````markdown\n'
                '```markdown\n'
                '## Example Only\n'
                '<a id="example-anchor"></a>\n'
                '```\n'
                '````\n'
                '## After\n',
                encoding='utf-8',
            )
            self.assertEqual(markdown_anchors(document), {'before', 'after'})

    def test_fenced_links_stay_hidden_until_a_compatible_closer(self):
        source = (
            '[Before](before.md)\n\n'
            '1. Examples\n\n'
            '    ~~~~markdown\n'
            '    ~~~\n'
            '    [Short fence example](missing-short.md)\n'
            '    ~~~~ trailing text\n'
            '    [Fence with info text](missing-info.md)\n'
            '    ```\n'
            '    [Different fence example](missing-marker.md)\n'
            '    ~~~~~ \t\n\n'
            '[After](after.md)\n'
        )
        self.assertEqual(without_code(source), '[Before](before.md)\n\n1. Examples\n\n\n[After](after.md)')


class MarkdownLinkTests(unittest.TestCase):
    def test_code_examples_are_skipped_but_links_with_code_labels_are_recognized(self):
        source = (
            'Inline `[example](missing.md)` and [`printf`](cli.md).\n'
            'Double ``[outer](missing-double.md) with `quoted` text`` and [After](after.md).\n\n'
            'Wrapped `first line\n[example](missing-wrapped.md)` then [Next](next.md).\n'
        )
        self.assertEqual(list(markdown_link_targets(source)), ['cli.md', 'after.md', 'next.md'])

    def test_unmatched_or_escaped_backticks_do_not_hide_links(self):
        source = (
            r'Escaped \` [Keep](escaped.md) `.' '\n\n'
            r'Even slashes \\`[Example](missing.md)` [Keep](even.md).' '\n\n'
            'Open ` here.\n'
            '## Heading [Keep](heading.md) `\n\n'
            'Open ` before a fence.\n'
            '~~~markdown\n[Fenced](missing-fenced.md)\n~~~\n'
            'After the fence [Keep](fence-neighbor.md) `.\n'
        )
        self.assertEqual(list(markdown_link_targets(source)),
                         ['escaped.md', 'even.md', 'heading.md', 'fence-neighbor.md'])


if __name__ == '__main__':
    unittest.main()
