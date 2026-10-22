#!/usr/bin/env python3
"""Full PTY relay, actual zsh and bounded output snapshots; no native GUI control."""
import json
import os
from pathlib import Path
import socket
import signal
import time
import unittest
import zle_binary_e2e as h


def snapshot(session):
    path=session.directory.joinpath('capture-endpoint').read_text()
    with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
        client.settimeout(.5)
        client.connect(path)
        client.sendall(b'Q\0\0')
        parts=[]
        while True:
            part=client.recv(32768)
            if not part:break
            parts.append(part)
        return json.loads(b''.join(parts))


class CaptureSessionTests(unittest.TestCase):
    def session(self):return h.ShellSession(capture_session=True)

    def command(self,s,command):
        previous=snapshot(s)
        s.send(command+'\r')
        result=[]
        def done():
            nonlocal result
            result=snapshot(s)
            return result!=previous and result and result[-1]['command']==command
        s.until(done,timeout=10,label='completed capture record')
        return result[-1]

    def test_actual_error_output_status_and_default_amber_context(self):
        s=self.session()
        try:
            # Bypass personal ls aliases so the platform error is deterministic.
            record=self.command(s,"command ls AMBER_NONEXISTENT_947")
            self.assertNotEqual(record['status'],0)
            self.assertIn('AMBER_NONEXISTENT_947',record['output'])
            self.assertIn('No such file',record['output'])
            self.assertEqual(record['cwd'],str(s.cwd))
            s.send(b'\x18\x01')
            s.until(lambda:'Was möchtest du tun?' in s.text(),label='Amber input')
            s.send(b'\x0b\t')
            s.until(lambda:'Terminalausschnitt' in s.text(),label='actual output attached')
            s.send(b'\x1b[B'*12)
            s.until(lambda:'No such file' in s.text(),label='actual error excerpt in exact context')
            s.send(b'x')
            s.until(lambda:'Kein neuer Zusatzkontext' in s.text(),label='Off disables commands and outputs')
            rows=s.screen.display
            top=next(i for i,row in enumerate(rows) if 'amber' in row and 'Ctrl-O' in row)
            self.assertNotIn('No such file','\n'.join(rows[top:]))
            s.send(b'\r');s.cancel()
            s.assert_modes()
            after=self.command(s,"printf 'AFTER_AMBER\\n'")
            self.assertIn('AFTER_AMBER',after['output'])
        finally:s.close()

    def test_more_than_three_outputs_survive_the_real_socket_and_widget_query(self):
        s=self.session()
        try:
            for n in range(8): self.command(s,f"printf 'CHAIN_STEP_{n}\\n'")
            records=snapshot(s)
            for n in range(8): self.assertTrue(any(f'CHAIN_STEP_{n}' in r['output'] for r in records))
            self.assertLessEqual(len(json.dumps(records).encode()),24*1024)
            s.send(b'\x18\x01')
            s.until(lambda:'Was möchtest du tun?' in s.text(),label='Amber input')
            s.send(b'\x0b\t')
            s.until(lambda:'Terminalausschnitt' in s.text(),label='Rust socket query accepts more than three records')
            s.send(b'\r');s.cancel();s.assert_modes()
        finally:s.close()

    def test_real_tty_and_shell_state_remain_intact(self):
        s=self.session()
        try:
            record=self.command(s,"[[ -t 0 && -t 1 && -t 2 ]] && printf 'TTY_OK\\n'")
            self.assertEqual(record['status'],0)
            self.assertIn('TTY_OK',record['output'])
            self.command(s,'mkdir child; cd child')
            record=self.command(s,'pwd')
            self.assertEqual(record['cwd'],str(s.cwd/'child'))
            self.assertEqual(record['output'].strip(),str(s.cwd/'child'))
        finally:s.close()

    def test_interactive_input_is_never_in_snapshot(self):
        s=self.session()
        try:
            command="printf 'INPUT_READY\\n'; read -r answer; printf '%s\\n' \"$answer\""
            s.send(command+'\r')
            s.until(lambda:b'INPUT_READY\r\n' in s.output,label='interactive read')
            s.send('PRIVATE_TYPED_VALUE_81\r')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']==command,label='interactive command ends')
            record=snapshot(s)[-1]
            self.assertEqual(record['output'],'')
            self.assertTrue(record['omitted_reason'])
            self.assertNotIn('PRIVATE_TYPED_VALUE_81',json.dumps(snapshot(s)))
        finally:s.close()

    def test_ctrl_c_resize_and_background_jobs(self):
        s=self.session()
        try:
            s.send('sleep 30\r');s.drain(.2);s.send(b'\x03')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']=='sleep 30',label='interrupted command ends')
            self.assertEqual(snapshot(s)[-1]['status'],130)
            s.resize(47,19);s.drain(.2)
            record=self.command(s,'stty size')
            self.assertEqual(record['output'].strip(),'19 47')
            self.command(s,'sleep 0.2 &')
            record=self.command(s,"printf 'AFTER_BACKGROUND\\n'")
            self.assertIn('AFTER_BACKGROUND',record['output'])
        finally:s.close()

    def test_output_without_final_newline_is_preserved(self):
        s=self.session()
        try:
            record=self.command(s,"printf 'UNTERMINATED_OUTPUT_91'")
            self.assertIn('UNTERMINATED_OUTPUT_91',record['output'])
        finally:s.close()

    def test_large_output_secret_filter_and_exit_status_are_bounded(self):
        s=self.session()
        try:
            record=self.command(s,"python3 -c \"print('large output harmless '+('x'*100+'\\\\n')*5000); print('FINAL_ERROR_CAPTURE')\"")
            self.assertLessEqual(len(record['output'].encode()),4096)
            self.assertTrue(record['truncated'])
            self.assertIn('FINAL_ERROR_CAPTURE',record['output'])
            self.command(s,'false')
            record=self.command(s,'print -r -- $?')
            self.assertEqual(record['output'].strip(),'1')
            s.send("printf 'API_KEY=synthetic-secret\\n'; printf 'after header\\n'\r")
            s.drain(.3)
            self.assertNotIn('synthetic-secret',json.dumps(snapshot(s)))
        finally:s.close()

    def test_job_control_and_clean_exit(self):
        s=self.session()
        try:
            s.send('sleep 30\r');s.drain(.2);s.send(b'\x1a')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']=='sleep 30',label='stopped foreground job')
            self.assertEqual(snapshot(s)[-1]['status'],128+signal.SIGTSTP)
            s.send('fg\r');s.drain(.2);s.send(b'\x03')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']=='fg',label='continued foreground job')
            self.assertEqual(snapshot(s)[-1]['status'],130)
            endpoint=Path(s.directory.joinpath('capture-endpoint').read_text())
            s.send('exit 0\r')
            s.until(lambda:s.poll() is not None,label='relay and shell exit')
            self.assertEqual(s.returncode,0)
            self.assertFalse(endpoint.exists())
        finally:s.close()

    def test_slow_terminal_after_child_exit_does_not_spin(self):
        import measure_terminal as measure
        s=self.session()
        try:
            s.send("python3 -c 'import os; os.write(1,b\"x\"*60000)'; exit 0\r")
            time.sleep(.2)  # Deliberately stop reading the outer PTY.
            first=measure.sample(s.pid)
            time.sleep(.4)
            second=measure.sample(s.pid)
            usage=(second['cpu_ns']-first['cpu_ns'])/1e9/(second['wall']-first['wall'])*100
            self.assertLess(usage,10,'relay spins on hung-up PTY under backpressure')
            s.until(lambda:s.poll() is not None,label='drained relay exit')
            self.assertEqual(s.returncode,0)
        finally:s.close()

    def test_multiline_percent_unicode_and_typeahead_privacy(self):
        s=self.session()
        try:
            command="printf '%s\\n' 'ä 100%'\nprintf done"
            s.send(b'\x1b[200~'+command.encode()+b'\x1b[201~\r')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']==command,label='exact multiline metadata')
            self.assertIn('ä 100%',snapshot(s)[-1]['output'])
            self.assertIn('done',snapshot(s)[-1]['output'])
            command="read -r answer; printf '%s\\n' \"$answer\""
            s.send(command+'\rPRIVATE_TYPEAHEAD_91\r')
            s.until(lambda:snapshot(s) and snapshot(s)[-1]['command']==command,label='typed-ahead interactive input')
            self.assertEqual(snapshot(s)[-1]['output'],'')
            self.assertNotIn('PRIVATE_TYPEAHEAD_91',json.dumps(snapshot(s)))
        finally:s.close()

    @unittest.skipUnless(os.environ.get('AMBER_TEST_REAL')=='1','opt in with AMBER_TEST_REAL=1')
    def test_real_model_diagnoses_actual_terminal_error(self):
        s=h.ShellSession(capture_session=True,simulate=False)
        try:
            self.command(s,'ls AMBER_NONEXISTENT_LIVE_72')
            s.send(b'\x18\x01')
            s.until(lambda:'Was möchtest du tun?' in s.text(),label='Amber with captured error')
            s.send('Warum ist mein letzter Terminalbefehl fehlgeschlagen?\r')
            s.until(lambda:s.status_text() in ['Fertig','Fehler'],timeout=120,label='real error diagnosis')
            self.assertEqual(s.status_text(),'Fertig',s.text())
            history=json.loads((s.config_directory/'amber-shell/history.json').read_text())
            reply=history['entries'][-1]
            self.assertRegex(reply['explanation'].lower(),r'exist|gefunden|fehlt|vorhand|datei|verzeichnis')
            print('Live error diagnosis:',reply['explanation'],'|',reply['command'])
            s.cancel();s.assert_modes()
        finally:s.close()


if __name__=='__main__':unittest.main(verbosity=2)
