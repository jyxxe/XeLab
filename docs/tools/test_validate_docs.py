import tempfile
from pathlib import Path
import unittest

from validate_docs import markdown_anchors


class MarkdownAnchorTests(unittest.TestCase):
    def test_github_headings_and_custom_anchors(self):
        with tempfile.TemporaryDirectory() as directory:
            document = Path(directory) / 'example.md'
            document.write_text(
                '# HostService\n'
                '## 微部署资源与联网设置\n'
                '## A [Link](other.md) &amp; `rpc_method`! ###\n'
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
                'hostservice', '微部署资源与联网设置', 'a-link--rpc_method',
                'CustomAnchor', 'hostservice-1', 'hostservice-1-1',
            })


if __name__ == '__main__':
    unittest.main()
