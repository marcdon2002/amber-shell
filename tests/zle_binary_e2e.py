#!/usr/bin/env python3
"""Actual Rust binary + zsh ZLE + emulated VT/PTY. No GUI/Apple Terminal claim.

Default: simulator regression suite. --real: also use official Codex for two
requests and execute only an inspected, narrowly allowlisted printf command.
--user-shell: source the existing .zshrc in a fresh zsh -f process, then replace
history with an isolated stack. Never writes to the user's configuration/history.
Requires pyte under work/testdeps, and macOS PTY ioctls (outside Codex sandbox).
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import signal
import struct
import sys
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'work/testdeps'))
import pyte
from wcwidth import wcwidth

BINARY = ROOT / 'target/debug/amber-shell'
PLUGIN = ROOT / 'shell/amber-shell.zsh'
SIM_COMMAND = "printf '%s\\n' 'amber-shell ready'"
MARKER = 'AMBER_ZLE_CONTEXT_7C42'
USER_SHELL = False
REAL = False


class VTScreen(pyte.HistoryScreen):
    def __init__(self, columns, lines, writer):
        super().__init__(columns, lines, history=2000)
        self.writer = writer

    def write_process_input(self, data):
        # pyte's CSI 6n parser reports the cursor at the exact query position,
        # including queries split over read() chunks.
        self.writer(data.replace('\x9b', '\x1b[').encode())

    @property
    def display(self):
        # pyte 0.8.2 can retain an orphaned wide-character stub after shrinking
        # and restoring HistoryScreen. Its display property indexes data[0]
        # even when data is empty. Read such orphan cells as blanks without
        # modifying the VT buffer or swallowing actual process failures.
        try:
            return super().display
        except IndexError:
            rows=[]
            found_orphan=False
            for y in range(self.lines):
                chars=[]
                skip_stub=False
                for x in range(self.columns):
                    if skip_stub:
                        skip_stub=False
                        continue
                    value=self.buffer[y][x].data
                    if value=='':
                        found_orphan=True
                        value=' '
                    assert sum(map(wcwidth,value[1:]))==0
                    skip_stub=wcwidth(value[0])==2
                    chars.append(value)
                rows.append(''.join(chars))
            if not found_orphan:
                raise
            return rows


class ShellSession:
    def __init__(self, simulate=True, config_directory=None, cwd=None, shell_history=None, codex_program=None, capture_session=False):
        self.temp = tempfile.TemporaryDirectory(prefix='amber-zle-e2e-')
        self.directory = Path(self.temp.name)
        self.cwd=Path(cwd).resolve() if cwd else self.directory
        self.config_directory = Path(config_directory) if config_directory else self.directory / 'config'
        self.config_directory.mkdir(parents=True, exist_ok=True)
        self.capture_file = self.directory / 'capture'
        self.cursor_file = self.directory / 'cursor'
        self.output = b''
        self.returncode = None
        self.directory.joinpath('README.md').write_text(
            f'# Controlled E2E project\nThe verification marker is {MARKER}.\n', encoding='utf-8')
        self.directory.joinpath('isolated-history').touch()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(self.cwd)
            os.environ.update(TERM='xterm-256color', LC_ALL='en_US.UTF-8')
            os.environ['XDG_CONFIG_HOME'] = str(self.config_directory)
            if codex_program: os.environ['AMBER_CODEX_BIN'] = str(codex_program)
            if capture_session:
                os.execv(str(BINARY), [str(BINARY),'session','--','/bin/zsh','-f'])
            os.execv('/bin/zsh', ['zsh', '-f'])
        self.screen = VTScreen(100, 30, self.send)
        self.stream = pyte.ByteStream(self.screen)
        self.resize(100, 30)
        self.until_bytes(b'\x1b[?2004h')
        startup = self.directory / 'startup.zsh'
        # .zshrc is sourced without printing it. History is reset immediately
        # afterwards and the tests choose only synthetic context/files.
        user_setup = ''
        if USER_SHELL:
            rc = Path(os.environ.get('ZDOTDIR', str(Path.home()))) / '.zshrc'
            if rc.is_file():
                user_setup = 'source ' + shlex.quote(str(rc)) + '\n'
        startup.write_text(
            'HISTFILE=' + shlex.quote(str(self.directory / 'isolated-history')) + '\n'
            'SAVEHIST=0\n' + user_setup +
            'print -r -- "${+ZSH} ${+functions[omz]} ${#PROMPT}" > ' + shlex.quote(str(self.directory / 'shell-setup')) + '\n' +
            'unsetopt SHARE_HISTORY INC_APPEND_HISTORY APPEND_HISTORY\n'
            'HISTFILE=' + shlex.quote(str(self.directory / 'isolated-history')) + '\n'
            'fc -p "$HISTFILE" 100 0\nSAVEHIST=0\n'
            'cd ' + shlex.quote(str(self.cwd)) + '\n'
            'export AMBER_SHELL_BIN=' + shlex.quote(str(BINARY)) + '\n'
            'export XDG_CONFIG_HOME=' + shlex.quote(str(self.config_directory)) + '\n'
            'export AMBER_SHELL_SIMULATE=' + ('1' if simulate else '0') + '\n'
            'source ' + shlex.quote(str(PLUGIN)) + '\n'
            'print -rn -- "$AMBER_SHELL_CAPTURE_SOCKET" > ' + shlex.quote(str(self.directory/'capture-endpoint')) + '\n'
            '_amber_test_capture() { print -rn -- "$BUFFER" > '+shlex.quote(str(self.capture_file))+'; print -r -- "$CURSOR" > '+shlex.quote(str(self.cursor_file))+'; zle redisplay; }\n'
            '_amber_test_seed() { IFS= read -r -d "" BUFFER < '+shlex.quote(str(self.directory/'seed'))+'; CURSOR=$(<'+shlex.quote(str(self.directory/'seed-cursor'))+'); zle redisplay; }\n'
            'zle -N _amber_test_capture\nzle -N _amber_test_seed\n'
            "bindkey '^X^D' _amber_test_capture\nbindkey '^X^B' _amber_test_seed\n"
            +''.join('print -rs -- '+shlex.quote(command)+'\n' for command in (shell_history if shell_history is not None else ['printf synthetic-history']))+
            "print -r -- AMBER_ZLE_READY\n", encoding='utf-8')
        self.send('source ' + shlex.quote(str(startup)) + '\r')
        self.until_bytes(b'AMBER_ZLE_READY\r\n')
        self.drain(.15)
        self.baseline = termios.tcgetattr(self.fd)
        self.shell_setup = self.directory.joinpath('shell-setup').read_text().strip()
        if USER_SHELL and not self.shell_setup.startswith('1 1 '):
            raise AssertionError('Expected existing Oh My Zsh setup to load')
        # Never retain or print startup output from the user's shell setup.
        self.output = b''

    def send(self, value):
        data = value.encode('utf-8') if isinstance(value, str) else value
        os.write(self.fd, data)

    def resize(self, columns, rows):
        self.screen.resize(rows, columns)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))

    def poll(self):
        if self.returncode is None:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.returncode = os.waitstatus_to_exitcode(status)
        return self.returncode

    def drain(self, duration=.05):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            if select.select([self.fd], [], [], min(.02, max(0, end-time.monotonic())))[0]:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                self.output += data
                self.stream.feed(data)

    def until(self, predicate, timeout=8, label='condition'):
        deadline = time.monotonic()+timeout
        while time.monotonic() < deadline:
            self.drain(.03)
            if predicate():
                return
            if self.poll() is not None:
                break
        # No raw shell output here: user-shell startup may contain private data.
        raise AssertionError(f'Timed out waiting for {label}; process={self.returncode}')

    def until_bytes(self, needle, timeout=8):
        self.until(lambda: needle in self.output, timeout, repr(needle))

    def text(self):
        return '\n'.join(self.screen.display)

    def status_text(self):
        rows=self.screen.display
        for y,row in enumerate(rows[:-1]):
            if 'amber' in row and 'Ctrl-O' in row:
                match=re.search(r'(?:^|\s)(Streaming|Choose model|Context|History|Waiting|Done|Error|Notice|Ready|Stream)(?=\s|$)',rows[y+1])
                return match.group(1) if match else ''
        return ''

    def open(self, context='a'):
        self.send(b'\x18\x01')
        self.until(lambda: 'What would you like to do?' in self.text(), label='direct question screen via default hotkey')
        self.send(b'\x0b')
        self.until(lambda: 'Context' in self.status_text(), label='context screen via default hotkey')
        self.send(context+'\r')
        self.until(lambda: 'What would you like to do?' in self.text(), label='chat input')

    def question(self, text, timeout=8):
        # Bracketed paste is also the correct contract for multi-line/unicode text.
        self.send(b'\x1b[200~'+text.encode()+b'\x1b[201~\r')
        self.until(lambda: ('> printf' in self.text() and 'Streaming' not in self.text())
                   or 'Error:' in self.text(), timeout, 'completed response')
        if 'Error:' in self.text():
            # These rows only contain test content after startup/history isolation.
            raise AssertionError('Provider error: '+self.text())
        self.drain(.1)

    def take(self):
        self.send(b'\x19')
        self.until(lambda: 'Ctrl-O' not in self.text(), label='return to shell')
        return self.capture()

    def capture(self):
        self.capture_file.unlink(missing_ok=True)
        self.cursor_file.unlink(missing_ok=True)
        self.send(b'\x18\x04')
        self.until(lambda: self.cursor_file.is_file(), label='ZLE capture')
        return self.capture_file.read_bytes(), int(self.cursor_file.read_text())

    def seed(self, value, cursor):
        self.directory.joinpath('seed').write_bytes(value.encode()+b'\0')
        self.directory.joinpath('seed-cursor').write_text(str(cursor))
        self.send(b'\x18\x02')
        self.drain(.05)

    def cancel(self, key=b'\x1b'):
        self.send(key)
        self.until(lambda: 'Ctrl-O' not in self.text(), label='cancel return')
        self.drain(.05)

    def assert_modes(self):
        if termios.tcgetattr(self.fd) != self.baseline:
            raise AssertionError('TTY modes were not restored to the ZLE baseline')

    def close(self):
        if self.poll() is None:
            os.kill(self.pid, signal.SIGHUP)
        deadline=time.monotonic()+3
        # PTY output must be drained during shutdown; waiting without reads can
        # deadlock terminal writes in macOS exit handlers.
        while self.poll() is None and time.monotonic()<deadline:
            self.drain(.03)
        if self.poll() is None:
            os.kill(self.pid, signal.SIGKILL)
            while self.poll() is None:
                self.drain(.03)
        os.close(self.fd)
        self.temp.cleanup()


class BinaryZleTests(unittest.TestCase):
    def test_executed_shell_command_reaches_default_model_request(self):
        with tempfile.TemporaryDirectory(prefix='amber-context-rpc-') as directory:
            root=Path(directory)
            program=root/'codex'
            capture=root/'request.json'
            program.write_text('''#!/usr/bin/env python3
import sys,json,pathlib
root=pathlib.Path(__file__).parent
if 'mcp' in sys.argv:
 print('[]');sys.exit(0)
def emit(value): print(json.dumps(value),flush=True)
for line in sys.stdin:
 message=json.loads(line);method=message['method'];p=message.get('params',{});i=message.get('id')
 if i is None: continue
 result={}
 if method=='account/read':result={'account':{'type':'chatgpt'},'requiresOpenaiAuth':True}
 if method=='config/read':result={'config':{}}
 if method=='model/list':result={'data':[{'id':'gpt-6-luna','model':'gpt-6-luna','displayName':'Luna','hidden':False,'isDefault':True}],'nextCursor':None}
 if method=='thread/start':
  (root/'instructions.json').write_text(json.dumps(p))
  result={'thread':{'id':'context-test'},'model':p['model'],'modelProvider':'openai','serviceTier':p['serviceTier']}
 if method=='turn/start':
  (root/'request.json').write_text(p['input'][0]['text'])
  emit({'id':i,'result':{'turn':{'id':'turn-test'}}})
  answer=json.dumps({'explanation':'Lists the contents.','command':'ls'})
  emit({'method':'item/agentMessage/delta','params':{'threadId':'context-test','turnId':'turn-test','itemId':'answer','delta':answer}})
  emit({'method':'item/completed','params':{'threadId':'context-test','turnId':'turn-test','item':{'type':'agentMessage','id':'answer','text':answer,'phase':'final_answer'}}})
  emit({'method':'turn/completed','params':{'threadId':'context-test','turn':{'id':'turn-test','status':'completed','error':None}}})
  continue
 emit({'id':i,'result':result})
''')
            program.chmod(0o700)
            s=ShellSession(simulate=False,codex_program=program,capture_session=True)
            try:
                command="printf '%s\\n' 'EXECUTED_CONTEXT_81'"
                start=len(s.output);s.send(command+'\r')
                s.until(lambda:b'EXECUTED_CONTEXT_81\r\n' in s.output[start:],label='actual shell command execution')
                s.send(b'\x18\x01')
                s.until(lambda:'What would you like to do?' in s.text(),label='direct question with default context')
                s.send('Wie liste ich alle Ordner auf?\r')
                s.until(lambda:capture.is_file(),label='captured actual provider request')
                s.until(lambda:'Done' in s.status_text(),label='complete fake provider response')
                request=json.loads(capture.read_text())
                self.assertIn(command,request['context'])
                self.assertNotIn(MARKER,request['context'],'ordinary shell questions must not scan project files')
                self.assertEqual(request['conversation'],[])
                instructions=json.loads((root/'instructions.json').read_text())
                self.assertIn('simplest',instructions['baseInstructions'])
                self.assertEqual(s.take(),(b'ls',2))
                s.assert_modes()
                web=s.cwd/'apps/web'
                web.mkdir(parents=True)
                (s.cwd/'package.json').write_text('{"packageManager":"pnpm@10.0.0","workspaces":["apps/*"]}')
                (web/'package.json').write_text('{"name":"web","scripts":{"dev":"vite --host"},"description":"OMIT_UNUSED_DESCRIPTION"}')
                capture.unlink()
                s.send(b'\x15\x18\x01')
                s.until(lambda:'What would you like to do?' in s.text(),label='reopened question')
                s.send('Wie starte ich das npm Web Projekt?\r')
                s.until(lambda:capture.is_file(),label='project provider request')
                s.until(lambda:'Done' in s.status_text(),label='project response')
                project=json.loads(capture.read_text())['context']
                self.assertIn('apps/web/package.json',project)
                self.assertIn('vite --host',project)
                self.assertIn('pnpm@10.0.0',project)
                self.assertIn('Terminal excerpt',project)
                self.assertNotIn('OMIT_UNUSED_DESCRIPTION',project)
                self.assertLess(len(project.encode()),8192)
                self.assertEqual(s.take(),(b'ls',2))
                s.assert_modes()
            finally:s.close()

    def test_reopened_widget_recalls_amber_and_shell_without_execution(self):
        shell_command="printf '%s\\n' 'ZLE_RECALL_42' > shell-executed"
        s=ShellSession(shell_history=[shell_command])
        try:
            question='AMBER_ZLE_RECALL_42'
            s.open();s.question(question);s.cancel()
            s.open()
            self.assertNotIn(question,s.text())
            s.send(b'\t')
            s.until(lambda:'History' in s.status_text(),label='reopened history focus')
            s.until(lambda:'amber-shell ready' in s.text(),label='saved Amber command')
            s.send(b'\x1b[A'*20)
            s.until(lambda:question in s.text(),label='saved Amber question')
            s.send(b'\x1b[D')
            s.until(lambda:'> '+shell_command in s.text(),label='shell source selected')
            s.send(b'\x1b[C')
            s.until(lambda:'> '+SIM_COMMAND in s.text(),label='Amber source selected')
            self.assertEqual(s.take(),(SIM_COMMAND.encode(),len(SIM_COMMAND)))
            s.open();s.send(b'\t')
            s.until(lambda:'History' in s.status_text(),label='second reopened history')
            s.send(b'\x1b[D')
            s.until(lambda:'> '+shell_command in s.text(),label='shell source selected again')
            s.send(b'\r')
            s.until(lambda:'Ctrl-O' not in s.text(),label='history Enter returned to ZLE')
            self.assertEqual(s.capture(),(shell_command.encode(),len(shell_command)))
            self.assertFalse((s.cwd/'shell-executed').exists())
            s.assert_modes()
        finally:s.close()

    def test_history_navigation_and_enter_return_to_editable_zle(self):
        s=ShellSession()
        try:
            s.open()
            for number in range(3):
                s.send(f'History-question-{number}\r')
                s.until(lambda:'Streaming' in s.text(),label='streaming start')
                s.until(lambda:'Done' in s.text() and '> printf' in s.text(),label='complete response')
            s.send(b'\t')
            s.until(lambda:'History' in s.status_text(),label='history focus')
            s.send(b'\x1b[A'*20)
            s.until(lambda:'You:   History-question-0' in s.text(),label='earlier history with arrows')
            s.send(b'\x1b[B'*20)
            s.until(lambda:'You:   History-question-2' in s.text(),label='latest history with arrows')
            s.send(b'\x1b[D\r')
            s.until(lambda:'Ctrl-O' not in s.text(),label='history Enter returns to ZLE')
            self.assertEqual(s.capture(),(SIM_COMMAND.encode(),len(SIM_COMMAND)))
            s.assert_modes()
        finally:s.close()

    def test_model_picker_escape_keeps_dialog_and_previous_zle_buffer(self):
        s=ShellSession()
        try:
            original='printf previous-buffer'
            s.seed(original,7)
            s.open()
            s.send(b'\x0f')
            s.until(lambda:'Choose model' in s.text(),label='model picker')
            s.send(b'\x1b')
            s.until(lambda:'What would you like to do?' in s.text(),label='escape returns from picker to chat')
            self.assertNotIn('Choose model',s.text())
            s.cancel()
            self.assertEqual(s.capture(),(original.encode(),7))
            s.assert_modes()
        finally:s.close()

    def test_real_binary_default_hotkey_exact_transfer_then_normal_enter(self):
        s=ShellSession()
        try:
            s.open();s.question('Gib einen harmlosen Testbefehl.')
            command,cursor=s.take()
            self.assertEqual(command,SIM_COMMAND.encode())
            self.assertEqual(cursor,len(SIM_COMMAND))
            s.assert_modes()
            start=len(s.output)
            s.send(b'\r')
            # OMZ emits an OSC title immediately before command stdout, so the
            # output line need not be preceded by a raw CRLF byte sequence.
            s.until(lambda: b'amber-shell ready\r\n' in s.output[start:], label='normal Enter printf execution')
            self.assertNotIn(b'\x1b[?1049h',s.output)
            self.assertNotIn(b'\x1b[2J',s.output)
            self.assertNotIn(b'\x1b[3J',s.output)
        finally:s.close()

    def test_escape_and_ctrl_c_preserve_unicode_buffer_and_cursor(self):
        s=ShellSession()
        try:
            original="printf '%s' 'ä é 😀'"
            for key in (b'\x1b',b'\x03'):
                s.seed(original,7)
                s.send(b'\x18\x01')
                s.until(lambda:'What would you like to do?' in s.text(),label='direct question')
                s.cancel(key)
                self.assertEqual(s.capture(),(original.encode(),7))
                s.assert_modes()
        finally:s.close()

    def test_twenty_open_cancel_cycles_leave_zle_usable(self):
        s=ShellSession()
        try:
            for i in range(20):
                value=f'printf cycle-{i}'
                s.seed(value,4)
                s.open()
                s.cancel()
                self.assertEqual(s.capture(),(value.encode(),4))
                s.assert_modes()
        finally:s.close()

    def test_streaming_resize_minimal_and_command_focus_preserve_result(self):
        s=ShellSession()
        try:
            s.open()
            s.send('Prüfe Unicode ä é 😀\r')
            s.drain(.1)
            s.resize(40,12);s.drain(.15)
            s.resize(10,3)
            s.until(lambda:'Enlarge' in s.text(),label='minimal layout')
            s.resize(100,30)
            s.until(lambda:'> printf' in s.text(),label='complete after resize')
            s.send(b'\t')
            s.until(lambda:'←→ command' in s.text(),label='command focus')
            s.resize(10,3);s.drain(.1);s.resize(100,30)
            s.until(lambda:'←→ command' in s.text(),label='command focus after resize')
            command,cursor=s.take()
            self.assertEqual(command,SIM_COMMAND.encode())
            self.assertEqual(cursor,len(SIM_COMMAND))
            s.assert_modes()
            self.assertNotIn(b'\x1b[2J',s.output)
            self.assertNotIn(b'\x1b[3J',s.output)
        finally:s.close()

    def test_real_codex_context_followup_transfer_and_allowlisted_execution(self):
        if not REAL: self.skipTest('enable --real for official Codex E2E')
        s=ShellSession(simulate=False)
        try:
            s.open(context='a')
            self.assertIn('GPT-6 Luna',s.text())
            s.send(b'\x0f')
            s.until(lambda:'Choose model' in s.text() and '(active)' in s.text(),
                    timeout=90,label='official model catalog with active selection')
            selected=[line.strip() for line in s.screen.display if line.strip().startswith('> ')]
            self.assertEqual(len(selected),1)
            self.assertIn('luna',selected[0].lower())
            self.assertIn('(active)',selected[0])
            s.send(b'\r')
            s.until(lambda:'Choose model' not in s.text(),label='save actual catalog selection')
            settings=json.loads((s.config_directory/'amber-shell/settings.json').read_text())
            self.assertEqual(settings['model'],'gpt-6-luna')
            s.question('Welcher verification marker steht in der mitgegebenen README? Gib genau einen printf-Befehl, der ausschließlich diesen Marker mit abschließendem Zeilenumbruch ausgibt. Verwende printf mit %s\\n als Format.',timeout=180)
            self.assertIn(MARKER,s.text())
            # A unique question sentinel ensures the second completion isn't
            # confused with the already visible first suggestion.
            s.send(b'\x1b[200~'+('Rückfrage: Verwende den Marker aus deiner vorherigen Antwort und hänge _FOLLOWUP an. Gib ausschließlich einen printf-Befehl mit Format %s\\n, der diesen erweiterten Marker ausgibt.').encode()+b'\x1b[201~\r')
            s.until(lambda:MARKER+'_FOLLOWUP' in s.text() and '> printf' in s.text()
                    and 'Streaming' not in s.text(),timeout=180,label='real follow-up completion')
            self.assertNotIn('Error:',s.text())
            command,cursor=s.take()
            text=command.decode()
            self.assertEqual(cursor,len(text))
            self.assertFalse(any(c in text for c in '\n\r;&|`$<>'))
            self.assertEqual(shlex.split(text),['printf','%s\\n',MARKER+'_FOLLOWUP'])
            s.assert_modes()
            start=len(s.output)
            s.send(b'\r')
            s.until(lambda: (MARKER+'_FOLLOWUP\r\n').encode() in s.output[start:],label='allowlisted real printf execution')
        finally:s.close()


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--real',action='store_true')
    parser.add_argument('--user-shell',action='store_true')
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--plugin',type=Path,default=PLUGIN)
    args,remaining=parser.parse_known_args()
    REAL=args.real;USER_SHELL=args.user_shell;BINARY=args.binary.resolve();PLUGIN=args.plugin.resolve()
    unittest.main(argv=[sys.argv[0]]+remaining,verbosity=2)
