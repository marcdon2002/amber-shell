#!/usr/bin/env python3
"""Real zsh/ZLE transport tests. The child stub isolates the Rust UI boundary."""
import os
import fcntl
import struct
import termios
import pathlib
import pty
import select
import shlex
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'shell/amber-shell.zsh'
INSTALLER = ROOT / 'scripts/install.sh'


class ZleSession:
    def __init__(self, directory, setup=''):
        self.directory = pathlib.Path(directory)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(directory)
            os.environ.update(TERM='xterm-256color', LC_ALL='en_US.UTF-8')
            os.execv('/bin/zsh', ['zsh', '-f'])
        self.output = b''
        self.resize(80, 24)
        self.until(b'\x1b[?2004h')
        self.send("unsetopt beep; PS1='READY> '; " + setup + '\n')
        self.until(b'\x1b[?2004h')

    def resize(self, columns, rows):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))

    def send(self, value):
        os.write(self.fd, value.encode() if isinstance(value, str) else value)

    def until(self, needle, timeout=5):
        start = len(self.output)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if needle in self.output[start:]:
                return self.output[start:]
            if select.select([self.fd], [], [], 0.05)[0]:
                try:
                    self.output += os.read(self.fd, 65536)
                except OSError:
                    break
        raise AssertionError(f'missing {needle!r}: {self.output[-1000:]!r}')

    def close(self):
        try:
            os.kill(self.pid, signal.SIGHUP)
        except ProcessLookupError:
            pass
        os.close(self.fd)
        os.waitpid(self.pid, 0)


class WidgetTests(unittest.TestCase):
    def run_widget(self, payload, exit_code=0, original="echo 'ursprünglich'", cursor=7, source_twice=False, history_entries=(), corrupt_terminal=False):
        with tempfile.TemporaryDirectory() as tmp:
            p = pathlib.Path(tmp)
            (p / 'payload').write_bytes(payload)
            (p / 'history-setup').write_text('HISTSIZE=100; SAVEHIST=0\n' + ''.join('print -s -- '+shlex.quote(x)+'\n' for x in history_entries))
            stub = p / 'stub.py'
            stub.write_text('''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
root = pathlib.Path(__file__).parent
history = pathlib.Path(args[args.index('--history-file')+1])
(root/'history-observed').write_bytes(history.read_bytes())
(root/'temp-parent').write_text(str(history.parent))
(root/'temp-mode').write_text(oct(history.parent.stat().st_mode & 0o777))
pathlib.Path(args[args.index('--result-file')+1]).write_bytes((root/'payload').read_bytes())
''' + ('import termios\nt=termios.tcgetattr(0)\nt[3] |= termios.ECHO\ntermios.tcsetattr(0,termios.TCSANOW,t)\n' if corrupt_terminal else '') + 'sys.exit(' + str(exit_code) + ')\n')
            stub.chmod(0o755)
            setup = f"export AMBER_SHELL_BIN={shlex.quote(str(stub))}; source {shlex.quote(str(PLUGIN))}; "
            if source_twice:
                setup += f'source {shlex.quote(str(PLUGIN))}; '
            setup += "source history-setup; "
            setup += "_test_amber() { BUFFER=" + shlex.quote(original) + f"; CURSOR={cursor}; amber-shell-widget; print -rn -- \"$BUFFER\" > returned; print -r -- \"$CURSOR\" > cursor; BUFFER=''; zle redisplay; }}; zle -N _test_amber; bindkey '^T' _test_amber"
            session = ZleSession(tmp, setup)
            try:
                terminal_before=termios.tcgetattr(session.fd)
                session.send(b'\x14')
                deadline = time.monotonic()+5
                while not (p/'cursor').exists() and time.monotonic()<deadline:
                    if select.select([session.fd],[],[],0.05)[0]:
                        session.output += os.read(session.fd,65536)
                self.assertTrue((p/'cursor').exists(),session.output[-1000:])
                result = (p/'returned').read_bytes()
                position = int((p/'cursor').read_text())
                self.assertTrue((p/'temp-parent').exists(), session.output[-1500:])
                if (p/'temp-parent').exists():
                    self.assertFalse(pathlib.Path((p/'temp-parent').read_text()).exists())
                    self.assertEqual((p/'temp-mode').read_text(),'0o700')
                self.last_history=(p/'history-observed').read_bytes()
                self.assertEqual(termios.tcgetattr(session.fd),terminal_before)
                self.assertFalse((p/'MUST_NOT_EXECUTE').exists())
                return result, position
            finally:
                session.close()

    def test_exact_success_unicode_quotes_and_final_newlines(self):
        command = "  printf '%s\\n' 'ä é 😀'\nprintf okay\n\n"
        result, cursor = self.run_widget(command.encode()+b'\0')
        self.assertEqual(result, command.encode())
        self.assertEqual(cursor, len(command))

    def test_cancel_restores_original_and_cursor(self):
        result, cursor = self.run_widget(b'ignored\0', exit_code=2)
        self.assertEqual(result, "echo 'ursprünglich'".encode())
        self.assertEqual(cursor,7)

    def test_error_restores_original_and_cursor(self):
        result, cursor = self.run_widget(b'partial',exit_code=1)
        self.assertEqual(result,"echo 'ursprünglich'".encode())
        self.assertEqual(cursor,7)

    def test_unterminated_and_trailing_data_are_rejected(self):
        for data in (b'command',b'command\0extra',b'command\0\0'):
            with self.subTest(data=data):
                result,cursor=self.run_widget(data)
                self.assertEqual(result,"echo 'ursprünglich'".encode())
                self.assertEqual(cursor,7)

    def test_controls_invalid_utf8_empty_and_oversize_rejected(self):
        for data in (b'\0', b'\x1b[2J\0', b'x\r\0', b'\xff\0', b'x'*16385+b'\0', '\u0085'.encode()+b'\0'):
            with self.subTest(data=data[:20]):
                result,_=self.run_widget(data)
                self.assertEqual(result,"echo 'ursprünglich'".encode())

    def test_does_not_execute_command_and_reloading_is_safe(self):
        command='touch MUST_NOT_EXECUTE'
        result,_=self.run_widget(command.encode()+b'\0',source_twice=True)
        self.assertEqual(result,command.encode())

    def test_history_is_bounded_and_oversize_entries_are_omitted(self):
        entries=[f'printf entry-{i:02d}-'+('x'*990) for i in range(50)]
        entries.append('printf oversized-'+('y'*9000))
        self.run_widget(b'printf okay\0', history_entries=entries)
        self.assertLessEqual(len(self.last_history),8192)
        received=self.last_history.rstrip(b'\0').split(b'\0')
        self.assertLessEqual(len(received),20)
        self.assertTrue(received[-1].startswith(b'printf entry-49-'))
        self.assertEqual(received, sorted(received))
        self.assertNotIn(b'oversized',self.last_history)

    def test_history_transports_latest_twenty_in_chronological_order(self):
        entries=[f'printf chronological-{i:02d}' for i in range(30)]
        self.run_widget(b'printf okay\0', history_entries=entries)
        received=self.last_history.rstrip(b'\0').split(b'\0')
        self.assertEqual(received, [f'printf chronological-{i:02d}'.encode() for i in range(10,30)])

    def test_error_restores_terminal_modes(self):
        self.run_widget(b'partial',exit_code=1,corrupt_terminal=True)

    def test_widget_refuses_non_zle_invocation(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=pathlib.Path(tmp)
            stub=p/'stub';stub.write_text('#!/bin/sh\ntouch '+shlex.quote(str(p/'ran'))+'\n');stub.chmod(0o755)
            session=ZleSession(tmp,f'export AMBER_SHELL_BIN={shlex.quote(str(stub))}; source {shlex.quote(str(PLUGIN))}; amber-shell-widget; print $? > status')
            try:
                self.assertFalse((p/'ran').exists())
                self.assertNotEqual(int((p/'status').read_text()),0)
            finally: session.close()

    def test_binding_conflict_preserves_existing_action(self):
        with tempfile.TemporaryDirectory() as tmp:
            setup=f"bindkey '^X^A' beginning-of-line; source {shlex.quote(str(PLUGIN))}; bindkey '^X^A' > binding"
            session=ZleSession(tmp,setup)
            try:
                deadline=time.monotonic()+3
                while not pathlib.Path(tmp,'binding').exists() and time.monotonic()<deadline: time.sleep(.02)
                self.assertIn('beginning-of-line',pathlib.Path(tmp,'binding').read_text())
            finally: session.close()


class InstallerTests(unittest.TestCase):
    def test_install_preserves_config_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=pathlib.Path(tmp)
            rc=p/'.zshrc'; original='# my existing setup\nexport TEST_VALUE=1\n'; rc.write_text(original)
            binary=p/'test-binary'; binary.write_text('#!/bin/sh\nexit 0\n');binary.chmod(0o755)
            args=['sh',str(INSTALLER),'--prefix',str(p/'.local'),'--zshrc',str(rc),'--zshenv',str(p/'.zshenv'),'--binary',str(binary)]
            first=subprocess.run(args,capture_output=True,text=True)
            self.assertEqual(first.returncode,0,first.stderr)
            text=rc.read_text();self.assertTrue(text.startswith(original))
            self.assertEqual(len(list(p.glob('.zshrc.amber-backup-*'))),1)
            subprocess.run(args,check=True,capture_output=True)
            self.assertEqual(rc.read_text(),text)
            self.assertTrue((p/'.local/bin/amber-shell').is_file())
            self.assertTrue((p/'.local/share/amber-shell/amber-shell.zsh').is_file())
            self.assertTrue((p/'.local/share/amber-shell/amber-session.zsh').is_file())
            self.assertIn('amber-session.zsh',(p/'.zshenv').read_text())
            self.assertEqual((p/'.zshenv').stat().st_mode & 0o777,0o600)


if __name__=='__main__': unittest.main(verbosity=2)
