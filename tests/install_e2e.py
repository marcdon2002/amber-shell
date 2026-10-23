#!/usr/bin/env python3
"""Installer checks in temporary homes; never touch personal startup files."""
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALLER = ROOT / 'scripts/install.sh'


class HomebrewSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.rc = self.root / '.zshrc'
        self.env = self.root / '.zshenv'
        self.rc.write_text('# personal prompt\nexport KEEP_ME=1\n')
        self.env.write_text('# personal environment\n')
        self.binary = self.root / 'binary'
        self.binary.write_text('#!/bin/sh\nexit 0\n')
        self.binary.chmod(0o755)
        self.prefix = self.root / 'opt' / 'amber-shell'

    def run_setup(self, prefix, *options):
        return subprocess.run(['sh', str(INSTALLER), '--prefix', str(prefix),
                               '--zshrc', str(self.rc), '--zshenv', str(self.env),
                               '--binary', str(self.binary), *options],
                              capture_output=True, text=True)

    def installed_prefix(self):
        result = self.run_setup(self.prefix)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_shell_only_preserves_package_files_and_repeated_setup(self):
        self.installed_prefix()
        before = {p: p.read_bytes() for p in self.prefix.rglob('*') if p.is_file()}
        config = (self.rc.read_bytes(), self.env.read_bytes())
        self.binary.unlink()  # Shell-only mode uses the installed binary.
        result = self.run_setup(self.prefix, '--shell-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(config, (self.rc.read_bytes(), self.env.read_bytes()))
        self.assertEqual(before, {p: p.read_bytes() for p in before})

    def test_migration_replaces_only_managed_blocks_with_backups(self):
        old = self.root / '.local'
        self.assertEqual(self.run_setup(old).returncode, 0)
        self.installed_prefix_files()
        before_rc, before_env = self.rc.read_bytes(), self.env.read_bytes()
        result = self.run_setup(self.prefix, '--shell-only', '--migrate')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('# personal prompt\nexport KEEP_ME=1\n', self.rc.read_text())
        self.assertIn('# personal environment\n', self.env.read_text())
        self.assertNotIn(str(old), self.rc.read_text() + self.env.read_text())
        self.assertIn(str(self.prefix), self.rc.read_text() + self.env.read_text())
        self.assertIn(before_rc, [p.read_bytes() for p in self.root.glob('.zshrc.amber-backup-*')])
        self.assertIn(before_env, [p.read_bytes() for p in self.root.glob('.zshenv.amber-backup-*')])
        again = self.run_setup(self.prefix, '--shell-only', '--migrate')
        self.assertEqual(again.returncode, 0, again.stderr)

    def installed_prefix_files(self):
        self.prefix.joinpath('bin').mkdir(parents=True)
        self.prefix.joinpath('bin/amber-shell').write_bytes(self.binary.read_bytes())
        self.prefix.joinpath('bin/amber-shell').chmod(0o755)
        share = self.prefix / 'share/amber-shell'
        share.mkdir(parents=True)
        for name in ['amber-shell.zsh', 'amber-session.zsh']:
            share.joinpath(name).write_bytes((ROOT / 'shell' / name).read_bytes())

    def test_custom_or_malformed_managed_blocks_leave_both_files_unchanged(self):
        for block in [
            '# >>> amber-shell >>>\necho custom\n# <<< amber-shell <<<\n',
            '# <<< amber-shell <<<\n# >>> amber-shell >>>\n',
            '# >>> amber-shell >>>\n',
            '# >>> amber-shell >>>\n# >>> amber-shell >>>\n# <<< amber-shell <<<\n',
            "# >>> amber-shell >>> \nexport AMBER_SHELL_BIN='/old/bin/amber-shell'\nsource '/old/share/amber-shell/amber-shell.zsh'\n# <<< amber-shell <<< \n",
        ]:
            with self.subTest(block=block):
                self.rc.write_text(block)
                before = (self.rc.read_bytes(), self.env.read_bytes())
                result = self.run_setup(self.prefix, '--migrate')
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(before, (self.rc.read_bytes(), self.env.read_bytes()))

    def test_shell_only_requires_complete_installed_package(self):
        result = self.run_setup(self.prefix, '--shell-only')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.rc.read_text(), '# personal prompt\nexport KEEP_ME=1\n')
        self.assertEqual(self.env.read_text(), '# personal environment\n')

    def test_invalid_second_config_parent_leaves_first_config_unchanged(self):
        self.root.joinpath('regular-file').write_text('not a directory')
        self.rc = self.root / 'regular-file' / '.zshrc'
        before = self.env.read_bytes()
        result = self.run_setup(self.prefix)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.env.read_bytes(), before)


if __name__ == '__main__':
    unittest.main(verbosity=2)
