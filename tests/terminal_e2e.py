"""Real binary + PTY. Synthetic input only; terminal cursor reports emulated."""
import os, pty, select, signal, tempfile, termios, time, fcntl, struct, unittest, json
import sys
import re
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "work/testdeps"))
import pyte

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / 'target/debug/amber-shell'

class VTScreen(pyte.Screen):
    def __init__(self, columns, lines, writer):
        super().__init__(columns, lines)
        self.writer=writer
    def write_process_input(self, data):
        self.writer(data.replace('\x9b','\x1b[').encode())

    def resize_with_reflow(self,columns,lines):
        """Controlled hard-line reflow approximation, not an Apple Terminal emulator.

        Existing nonempty rows wrap at the new width, and the cursor follows its
        old cell. This exercises the shift omitted by pyte's ordinary crop-only
        resize. A roomy test viewport avoids ambiguous scrollback reconstruction.
        """
        old_x,old_y=self.cursor.x,self.cursor.y
        occupied=[y for y in range(self.lines) if any(self.buffer[y][x].data.strip() for x in range(self.columns))]
        last=max(occupied+[old_y])
        rows=[]
        cursor=None
        starts={}
        for y in range(last+1):
            starts[y]=len(rows)
            cells=[self.buffer[y][x] for x in range(self.columns)]
            used=max([x+1 for x,cell in enumerate(cells) if cell.data.strip()]+[1])
            if y==old_y:
                used=max(used,old_x+1)
                cursor=(old_x%columns,len(rows)+old_x//columns)
            rows.extend([cells[start:start+columns] for start in range(0,used,columns)])
        removed=max(0,len(rows)-lines)
        self.resize(lines,columns)
        self.buffer.clear()
        for y,cells in enumerate(rows[removed:]):
            for x,cell in enumerate(cells):self.buffer[y][x]=cell
        self.cursor.x=cursor[0]
        self.cursor.y=max(0,cursor[1]-removed)
        for saved in self.savepoints:
            saved.cursor.y=max(0,min(lines-1,starts.get(saved.cursor.y,0)+saved.cursor.x//columns-removed))
            saved.cursor.x%=columns

class Child:
    def __init__(self,pid): self.pid=pid;self.returncode=None
    def poll(self):
        if self.returncode is None:
            pid,status=os.waitpid(self.pid,os.WNOHANG)
            if pid:self.returncode=os.waitstatus_to_exitcode(status)
        return self.returncode
    def wait(self,timeout=5):
        end=time.monotonic()+timeout
        while self.poll() is None and time.monotonic()<end:time.sleep(.02)
        if self.returncode is None:raise TimeoutError('child did not exit')
        return self.returncode
    def kill(self):os.kill(self.pid,signal.SIGKILL)

class Session:
    def __init__(self, seeded=False, config_directory=None, review=True, cwd=None, shell_history=None):
        self.temp = tempfile.TemporaryDirectory()
        self.result = Path(self.temp.name) / 'result'
        self.config_directory = Path(config_directory) if config_directory else Path(self.temp.name) / 'config'
        self.config_directory.mkdir(parents=True, exist_ok=True)
        self.cwd=Path(cwd).resolve() if cwd else Path(self.temp.name)
        arguments=[str(BIN),'--simulate','--result-file',str(self.result)]
        if shell_history is not None:
            history_file=Path(self.temp.name)/'synthetic-history'
            history_file.write_bytes(b''.join(command.encode()+b'\0' for command in shell_history))
            arguments+=['--history-file',str(history_file)]
        pid,self.master=pty.fork()
        if pid==0:
            os.chdir(self.cwd)
            os.environ['TERM']='xterm-256color'
            os.environ['XDG_CONFIG_HOME']=str(self.config_directory)
            os.execv(str(BIN),arguments)
        self.slave=self.master
        self.proc=Child(pid)
        self.old = termios.tcgetattr(self.master)
        self.screen=VTScreen(80,24,self.send)
        self.stream=pyte.ByteStream(self.screen)
        self.resize(80,24)
        # Leave exactly 18 physical rows below the shell sentinel: the enlarged
        # 24-row reservation fits without legitimate terminal scrolling, so
        # preservation checks can still assert the sentinel's exact position.
        if seeded:self.stream.feed(b'\x1b[6;1HX\x1b[7;1H')
        self.output = b''
        self.wait_screen('Frag nach einem Befehl.')
        if review:
            self.send(b'\x0b')
            self.wait_status('Kontext')
    def resize(self,w,h):
        self.screen.resize(h,w)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH',h,w,0,0))
    def reflow_resize(self,w,h):
        self.screen.resize_with_reflow(w,h)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH',h,w,0,0))
    def read(self,seconds=.2):
        until=time.monotonic()+seconds
        while time.monotonic()<until:
            if select.select([self.master],[],[],.02)[0]:
                try: chunk=os.read(self.master,65536)
                except OSError: break
                if not chunk:break
                self.output+=chunk
                self.stream.feed(chunk)
        return self.output
    def send(self,text): os.write(self.master,text if isinstance(text,bytes) else text.encode())
    def text(self): return '\n'.join(self.screen.display)
    def status_text(self):
        rows=self.screen.display
        for y,row in enumerate(rows[:-1]):
            if 'amber' in row and 'Ctrl-O' in row:
                match=re.search(r'(?:^|\s)(Antwort läuft|Modellwahl|Kontext|Verlauf|Wartet|Fertig|Fehler|Hinweis|Bereit|Stream)(?=\s|$)',rows[y+1])
                return match.group(1) if match else ''
        return ''
    def wait_status(self,needle,timeout=8):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            self.read(.03)
            if needle in self.status_text():return
            if self.proc.poll() is not None:break
        raise AssertionError(f'missing header status {needle!r}')
    def wait_screen(self,needle,timeout=8):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            self.read(.03)
            if needle in self.text():return
            if self.proc.poll() is not None:break
        raise AssertionError(f'missing screen text {needle!r}')
    def wait_for(self,needle,timeout=8):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            self.read(.05)
            if needle in self.output:return
            if self.proc.poll() is not None:break
        raise AssertionError(f'missing {needle!r}; last output {self.output[-1200:]!r}')
    def wait(self,timeout=5):
        until=time.monotonic()+timeout
        while self.proc.poll() is None and time.monotonic()<until:self.read(.03)
        if self.proc.poll() is None:raise TimeoutError('child did not exit; output='+repr(self.output[-300:]))
    def close(self):
        if self.proc.poll() is None:self.proc.kill()
        self.wait(); os.close(self.master);self.temp.cleanup()

class TerminalTests(unittest.TestCase):
    def test_leaving_history_never_transfers_a_hidden_old_selection(self):
        old_command="printf '%s\\n' 'OLD_SHELL_SELECTION'"
        expected=b"printf '%s\\n' 'amber-shell bereit'\0"
        for departure in ('tab','typing','paste'):
            with self.subTest(departure=departure):
                s=Session(review=False,shell_history=[old_command])
                try:
                    s.send(b'\t');s.wait_status('Verlauf')
                    s.wait_screen('> '+old_command)
                    if departure=='tab':
                        s.send(b'\t');s.wait_screen('Frag nach einem Befehl.')
                        s.send('Neue Frage nach Tab\r')
                    elif departure=='typing':
                        s.send('Neue Frage nach Tippen\r')
                    else:
                        s.send(b'\x1b[200~Neue Frage nach Paste\x1b[201~\r')
                    s.wait_status('Fertig')
                    self.assertNotIn(old_command,s.text())
                    s.send(b'\x19');s.wait()
                    self.assertEqual(s.result.read_bytes(),expected)
                finally:s.close()

    def test_reopen_same_directory_recalls_amber_and_shell_commands(self):
        with tempfile.TemporaryDirectory(prefix='amber-recall-') as shared:
            project=Path(shared)/'project';project.mkdir()
            config=Path(shared)/'config'
            shell_command="printf '%s\\n' 'SHELL_RECALL_42' > shell-executed"
            question='AMBER_RECALL_42'
            command=b"printf '%s\\n' 'amber-shell bereit'\0"
            first=Session(review=False,cwd=project,config_directory=config,shell_history=[shell_command])
            try:
                first.send(question+'\r');first.wait_status('Fertig')
                first.send(b'\x1b');first.wait()
                self.assertFalse(first.result.exists())
            finally:first.close()
            recalled=Session(review=False,cwd=project,config_directory=config,shell_history=[shell_command])
            try:
                self.assertNotIn(question,recalled.text())
                recalled.send(b'\t');recalled.wait_status('Verlauf')
                recalled.wait_screen('amber-shell bereit')
                recalled.send(b'\x1b[A'*20);recalled.wait_screen(question)
                recalled.send(b'\x1b[D');recalled.wait_screen('> '+shell_command)
                recalled.send(b'\x1b[C');recalled.wait_screen("> printf '%s\\n' 'amber-shell bereit'")
                recalled.send(b'\x19');recalled.wait()
                self.assertEqual(recalled.result.read_bytes(),command)
            finally:recalled.close()
            shell=Session(review=False,cwd=project,config_directory=config,shell_history=[shell_command])
            try:
                shell.send(b'\t');shell.wait_status('Verlauf')
                shell.send(b'\x1b[D');shell.wait_screen('> '+shell_command)
                shell.send(b'\r');shell.wait()
                self.assertEqual(shell.result.read_bytes(),shell_command.encode()+b'\0')
                self.assertFalse((project/'shell-executed').exists())
            finally:shell.close()

    def assert_compact(self,s):
        occupied=[index for index,row in enumerate(s.screen.display) if row.strip()]
        self.assertTrue(occupied)
        cap=min(20,s.screen.lines*3//4) if s.screen.lines>=18 else s.screen.lines
        self.assertLessEqual(occupied[-1]-occupied[0]+1,cap)

    def test_dialog_grows_with_content_and_respects_physical_height(self):
        s=Session(review=False)
        try:
            s.resize(80,40);s.read(.2)
            def occupied_height():
                rows=[index for index,row in enumerate(s.screen.display) if row.strip()]
                return rows[-1]-rows[0]+1
            initial=occupied_height()
            for number in range(3):
                s.send(f'Adaptive Inhaltsfrage {number}\r')
                s.wait_status('Antwort läuft');s.wait_status('Fertig')
                self.assert_compact(s)
            self.assertGreater(occupied_height(),initial)
            self.assertGreater(occupied_height(),14,'long dialog must use the newly available body space')
            s.resize(80,24);s.read(.3)
            self.assert_compact(s)
            s.send(b'\x19');s.wait()
            self.assertEqual(s.result.read_bytes(),b"printf '%s\\n' 'amber-shell bereit'\0")
        finally:s.close()

    def test_launch_starts_at_question_and_context_is_optional(self):
        s=Session(review=False)
        try:
            self.assertNotIn('Kontext',s.status_text())
            s.send(b'\x0b');s.wait_status('Kontext')
            s.send(b'\x1b');s.wait_screen('Frag nach einem Befehl.')
            self.assertIsNone(s.proc.poll())
            s.send(b'\x1b');s.wait()
            self.assertEqual(s.proc.returncode,2)
        finally:s.close()

    def test_width_reflow_redraw_clears_only_owned_rows_and_restores_shell(self):
        s=Session(seeded=True)
        try:
            s.wait_status('Kontext');s.resize(80,40);s.read(.2)
            s.send(b'a\r');s.wait_screen('Frag nach einem Befehl.')
            s.send('Prüfe die vorhandenen Dateien im Projekt\r');s.wait_screen('Fertig')
            self.assertEqual(s.screen.display[5][0],'X')
            s.reflow_resize(40,40);s.read(.4)
            text=s.text()
            self.assertEqual(text.count('amber'),2,'one brand and one command, no stale reflow fragments')
            self.assertEqual(s.screen.display[5][0],'X')
            s.send(b'\x1b');s.wait();s.read(.1)
            self.assertEqual(s.screen.display[5][0],'X')
            remaining=[row.strip() for row in s.screen.display if row.strip()]
            self.assertEqual(remaining,['X'],'exit must clear all reflowed UI rows without erasing prior shell text')
        finally:s.close()

    def test_idle_and_streaming_width_height_reflow_leave_no_fragments(self):
        for streaming in (False,True):
            with self.subTest(streaming=streaming):
                s=Session(seeded=True,review=False)
                try:
                    s.resize(80,50);s.read(.1)
                    if streaming:
                        s.send('Prüfe Resize während der Antwort\r');s.wait_screen('Antwort läuft')
                    for width,height in [(40,40),(60,36),(80,50)]:
                        s.reflow_resize(width,height);s.read(.18)
                        self.assertEqual(sum('amber' in row and 'Ctrl-O' in row for row in s.screen.display),1)
                        self.assertIn('X',[row.strip() for row in s.screen.display])
                    if streaming:s.wait_screen('Fertig')
                    s.send(b'\x1b');s.wait();s.read(.1)
                    self.assertEqual([row.strip() for row in s.screen.display if row.strip()],['X'])
                    self.assertNotIn(b'\x1b[2J',s.output)
                    self.assertNotIn(b'\x1b[3J',s.output)
                finally:s.close()

    def test_fast_picker_draft_cancel_persist_and_unsupported_model(self):
        with tempfile.TemporaryDirectory(prefix='amber-fast-test-') as config:
            s=Session(config_directory=config,review=False)
            settings=Path(config)/'amber-shell/settings.json'
            try:
                s.send(b'\x0f');s.wait_screen('Fast aus')
                s.send(b'\x06');s.wait_screen('Fast an')
                s.send(b'\x1b');s.wait_screen('Frag nach einem Befehl.')
                self.assertFalse(settings.exists(),'Escape must discard the draft')
                s.send(b'\x0f');s.wait_screen('Fast aus')
                s.send(b'\x06\r');s.wait_screen('Modell gespeichert')
                self.assertEqual(json.loads(settings.read_text()),{'model':'gpt-6-luna','fast':True,'context':'automatic'})
                s.send(b'\x1b');s.wait()
            finally:s.close()
            reopened=Session(config_directory=config,review=False)
            try:
                self.assertIn('Fast',reopened.text())
                reopened.send(b'\x0f');reopened.wait_screen('Fast an')
                reopened.send(b'\x1b[B');reopened.wait_screen('Fast für dieses Modell nicht verfügbar')
                reopened.send(b'\r');reopened.wait_screen('Fast nicht verfügbar')
                self.assertEqual(json.loads(settings.read_text()),{'model':'gpt-6-sol','fast':False,'context':'automatic'})
                reopened.send(b'\x1b');reopened.wait()
            finally:reopened.close()

    def test_context_modes_persist_and_raw_preview_matches_the_draft(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory,'apps/web').mkdir(parents=True)
            Path(directory,'apps/web/package.json').write_text('{"scripts":{"dev":"vite"}}')
            config=Path(directory,'config')
            s=Session(cwd=directory,config_directory=config,shell_history=["printf 'COMMAND_CONTEXT_DEFAULT'"],review=False)
            try:
                s.send('npm web dev?');s.send(b'\x0b');s.wait_screen('A  Automatisch')
                s.send(b'\t');s.send(b'\x1b[B'*8);s.wait_screen('vite')
                s.send(b't');s.send(b'\x1b[B'*4);s.wait_screen('COMMAND_CONTEXT_DEFAULT')
                self.assertNotIn('vite',s.text())
                s.send(b'x');s.wait_screen('Kein neuer Zusatzkontext')
                s.send(b'\t');s.send(b'\r');s.wait_screen('npm web dev?')
                self.assertEqual(json.loads((config/'amber-shell/settings.json').read_text())['context'],'off')
                s.send(b'\x1b');s.wait()
            finally:s.close()
            reopened=Session(cwd=directory,config_directory=config)
            try:
                reopened.wait_screen('Kein neuer Zusatzkontext')
                reopened.send(b'a');reopened.wait_screen('A  Automatisch')
                reopened.send(b'\r\x1b');reopened.wait()
                self.assertEqual(json.loads((config/'amber-shell/settings.json').read_text())['context'],'automatic')
            finally:reopened.close()

    def test_context_save_error_is_visible_and_does_not_apply_unsaved_mode(self):
        s=Session()
        try:
            directory=s.config_directory/'amber-shell'
            directory.mkdir(exist_ok=True)
            (directory/'settings.json').write_text('broken')
            s.send(b'x');s.wait_screen('Einstellung nicht gespeichert')
            self.assertIn('A  Automatisch',s.text())
            self.assertNotIn('Kein neuer Zusatzkontext',s.text())
            s.send(b'\r\x1b');s.wait()
        finally:s.close()

    def test_idle_sends_no_redraw_bytes(self):
        s=Session()
        try:
            s.wait_status('Kontext');s.read(.1)
            before=len(s.output)
            s.read(.8)
            self.assertEqual(len(s.output),before,'idle context must not redraw')
            s.send(b'a\r');s.wait_screen('Frag nach einem Befehl.');s.read(.1)
            before=len(s.output)
            s.read(.8)
            self.assertEqual(len(s.output),before,'idle input must not animate')
        finally:s.close()

    def test_active_logo_animates_then_stays_finished_without_redraw(self):
        s=Session()
        try:
            s.wait_status('Kontext');s.send(b'a\r');s.wait_screen('Frag nach einem Befehl.')
            s.send('Prüfe den aktiven Status\r');s.wait_screen('Antwort läuft')
            headers=set()
            deadline=time.monotonic()+8
            while time.monotonic()<deadline and 'Fertig' not in s.text():
                for y,row in enumerate(s.screen.display):
                    if 'amber' in row and 'Ctrl-O' in row:
                        logo_end=row.index('amber')
                        # Only the signet band left of the brand/status text;
                        # status changes must not count as animation evidence.
                        headers.add(tuple((cell.data,cell.fg,cell.bg,cell.bold)
                                          for logo_y in range(y,min(y+2,s.screen.lines))
                                          for x in range(logo_end)
                                          for cell in [s.screen.buffer[logo_y][x]]))
                s.read(.03)
            self.assertGreaterEqual(len(headers),2,'active logo must visibly advance')
            s.wait_screen('Fertig')
            s.wait_screen('> printf')
            s.read(.1)
            before=len(s.output)
            finished=s.text()
            s.read(.8)
            self.assertEqual(len(s.output),before,'completed reply must stop timer redraws')
            self.assertEqual(s.text(),finished)
            self.assertIn('Fertig',s.text())
        finally:s.close()

    def test_model_picker_escape_then_dialog_escape(self):
        s=Session(review=False)
        try:
            s.wait_screen('GPT-6 Luna')
            self.assert_compact(s)
            s.send(b'\x0f');s.wait_screen('Modellwahl')
            self.assert_compact(s)
            s.send(b'\x1b');s.wait_screen('Frag nach einem Befehl.')
            self.assertIsNone(s.proc.poll())
            self.assertNotIn('Modellwahl',s.text())
            s.send(b'\x1b');s.wait()
            self.assertEqual(s.proc.returncode,2)
            self.assertFalse(s.result.exists())
        finally:s.close()

    def test_model_picker_arrow_enter_persists_for_next_process(self):
        with tempfile.TemporaryDirectory(prefix='amber-config-test-') as config:
            s=Session(config_directory=config)
            try:
                s.wait_screen('GPT-6 Luna')
                s.send(b'\x0f');s.wait_screen('GPT-6 Sol')
                s.send(b'\x1b[B\r');s.wait_screen('Modell gespeichert')
                saved=json.loads((Path(config)/'amber-shell/settings.json').read_text())
                self.assertEqual(saved['model'],'gpt-6-sol')
                s.send(b'\x03');s.wait()
            finally:s.close()
            reopened=Session(config_directory=config)
            try:
                reopened.wait_screen('GPT-6 Sol')
                reopened.send(b'\x03');reopened.wait()
            finally:reopened.close()

    def test_history_tab_arrows_and_enter_without_page_keys(self):
        s=Session()
        try:
            s.wait_status('Kontext');s.send(b'a\r');s.wait_screen('Frag nach einem Befehl.')
            for number in range(3):
                s.send(f'Verlauf-Frage-{number}\r')
                s.wait_screen('Antwort läuft')
                s.wait_screen('Fertig')
                s.wait_screen('> printf')
                self.assert_compact(s)
            s.send(b'\t');s.wait_status('Verlauf')
            s.send(b'\x1b[A'*20);s.wait_screen('Du:    Verlauf-Frage-0')
            self.assertNotIn('PgUp',s.text())
            s.send(b'\x1b[B'*20);s.wait_screen('Du:    Verlauf-Frage-2')
            s.send(b'\x1b[D');s.read(.1)
            s.send(b'\r');s.wait()
            self.assertEqual(s.proc.returncode,0)
            self.assertEqual(s.result.read_bytes(),b"printf '%s\\n' 'amber-shell bereit'\0")
        finally:s.close()

    def test_inline_stream_resize_transfer_and_cleanup(self):
        s=Session()
        try:
            s.wait_for(b'amber'); s.send(b'a\r');s.read();s.send('zeige das Verzeichnis\r')
            s.read(.2);s.resize(40,12);s.read(.2);s.resize(10,3);s.read(.2);s.resize(80,24)
            s.wait_screen('> printf');s.send(b'\x19');s.wait(timeout=5);s.read()
            self.assertEqual(s.proc.returncode,0)
            self.assertEqual(s.result.read_bytes(),b"printf '%s\\n' 'amber-shell bereit'\0")
            self.assertEqual(termios.tcgetattr(s.slave),s.old)
            self.assertNotIn(b'\x1b[?1049h',s.output)
            self.assertNotIn(b'\x1b[3J',s.output)
            self.assertNotIn(b'\x1b[2J',s.output)
        finally:s.close()
    def test_narrow_minimal_resize_does_not_clear_prior_shell_lines(self):
        s=Session(seeded=True)
        try:
            s.wait_for(b'amber');s.send(b'a\r');s.read(.2)
            s.resize(10,24);s.read(.5);s.resize(80,24);s.read(.5)
            self.assertEqual(s.screen.display[5][0],'X')
            s.send(b'\x1b');s.wait(timeout=5)
        finally:s.close()
    def test_burst_resize_keeps_shell_sentinel_before_next_draw(self):
        s=Session(seeded=True)
        try:
            s.wait_for(b'amber');s.send(b'a\r');s.read(.2)
            for _ in range(25):
                s.send('x');s.read(.001);s.resize(79,24);s.read(.001);s.read(.001);s.resize(80,24);s.read(.07)
                self.assertEqual(s.screen.display[5][0],'X')
            s.send(b'\x1b');s.wait()
        finally:s.close()
    def test_cancel_during_stream_restores_tty_and_returns_no_command(self):
        s=Session()
        try:
            s.wait_for(b'amber');s.send(b'a\r');s.read();s.send('frage\r');s.read(.1);s.send(b'\x03');s.wait(timeout=5)
            self.assertEqual(s.proc.returncode,2)
            self.assertFalse(s.result.exists())
            self.assertEqual(termios.tcgetattr(s.slave),s.old)
            self.assertNotIn(b'Fertig',s.output,'cancellation must not report completion')
        finally:s.close()

if __name__=='__main__':unittest.main()
