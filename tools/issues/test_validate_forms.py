"""Regression checks for required public issue-form controls."""
import contextlib
import io
from pathlib import Path
import shutil
import tempfile
import unittest

import yaml

from validate import ROOT, read, validate


class IssueFormValidationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        shutil.copytree(ROOT / '.github', self.root / '.github')

    def edit(self, filename, field_id, mutate):
        path = self.root / '.github/ISSUE_TEMPLATE' / filename
        form = read(path)
        field = next(item for item in form['body'] if item.get('id') == field_id)
        mutate(field)
        path.write_text(yaml.safe_dump(form, allow_unicode=True, sort_keys=False), encoding='utf-8')

    def validate_silently(self):
        with contextlib.redirect_stdout(io.StringIO()):
            validate(self.root)

    def test_existing_forms_are_valid(self):
        self.validate_silently()

    def test_security_disclosure_must_be_required_dropdown(self):
        def as_input(field):
            field['type'] = 'input'
            field['attributes'].pop('options')
        self.edit('04-security.yml', 'disclosure', as_input)
        with self.assertRaises(ValueError):
            self.validate_silently()

    def test_security_disclosure_keeps_private_channel_choice(self):
        def remove_private_choice(field):
            field['attributes']['options'] = [option for option in field['attributes']['options']
                                              if not option.startswith('请求私密报告渠道')]
        self.edit('04-security.yml', 'disclosure', remove_private_choice)
        with self.assertRaises(ValueError):
            self.validate_silently()

    def test_security_disclosure_requires_a_choice(self):
        self.edit('04-security.yml', 'disclosure',
                  lambda field: field['validations'].update(required=False))
        with self.assertRaises(ValueError):
            self.validate_silently()

    def test_security_private_option_cannot_reverse_its_scope(self):
        def reverse_scope(field):
            options = field['attributes']['options']
            index = next(i for i, option in enumerate(options) if option.startswith('请求私密报告渠道'))
            options[index] = '请求私密报告渠道（允许公开细节）'
        self.edit('04-security.yml', 'disclosure', reverse_scope)
        with self.assertRaises(ValueError):
            self.validate_silently()

    def test_security_confidentiality_confirmation_is_required(self):
        text = '确认标题、正文和附件不含未公开漏洞细节，必要时仅请求私密渠道。'
        for mutation in ('missing', 'reversed', 'optional'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                shutil.copytree(ROOT / '.github', root / '.github')
                path = root / '.github/ISSUE_TEMPLATE/04-security.yml'
                form = read(path)
                field = next(item for item in form['body'] if item.get('id') == 'checks')
                options = field['attributes']['options']
                index = next(i for i, option in enumerate(options) if option['label'] == text)
                if mutation == 'missing':
                    del options[index]
                elif mutation == 'reversed':
                    options[index]['label'] = '确认标题、正文和附件可以包含未公开漏洞细节。'
                else:
                    options[index]['required'] = False
                path.write_text(yaml.safe_dump(form, allow_unicode=True, sort_keys=False), encoding='utf-8')
                with self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                    validate(root)

    def test_checks_must_be_nonempty_required_checkboxes(self):
        for filename in ('01-bug.yml', '04-security.yml'):
            for mutation in ('input', 'empty', 'optional'):
                with self.subTest(filename=filename, mutation=mutation), tempfile.TemporaryDirectory() as temp:
                    root = Path(temp)
                    shutil.copytree(ROOT / '.github', root / '.github')
                    path = root / '.github/ISSUE_TEMPLATE' / filename
                    form = read(path)
                    field = next(item for item in form['body'] if item.get('id') == 'checks')
                    if mutation == 'input':
                        field['type'] = 'input'
                        field['attributes'].pop('options')
                        field['validations'] = {'required': True}
                    elif mutation == 'empty':
                        field['attributes']['options'] = []
                    else:
                        field['attributes']['options'][0]['required'] = False
                    path.write_text(yaml.safe_dump(form, allow_unicode=True, sort_keys=False), encoding='utf-8')
                    with self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                        validate(root)

    def test_unrelated_labels_and_extra_options_can_change(self):
        def reword(field):
            field['attributes']['label'] = '公开范围（请依照安全政策选择）'
            field['attributes']['options'][0] = '公开防御性加固'
            field['attributes']['options'].append('其他已批准的公开范围')
            field['attributes']['options'].reverse()
        self.edit('04-security.yml', 'disclosure', reword)
        def revise_checks(field):
            options = field['attributes']['options']
            options[0]['label'] = '确认已检查重复工作'
            options.append({'label': '补充检查', 'required': True})
            options.reverse()
        self.edit('04-security.yml', 'checks', revise_checks)
        self.validate_silently()


if __name__ == '__main__':
    unittest.main()
