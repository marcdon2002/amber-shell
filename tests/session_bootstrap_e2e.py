#!/usr/bin/env python3
"""Isolated startup/installer checks; personal shell files are never modified."""
import os
from pathlib import Path
import pty
import select
import shlex
import signal
import subprocess
import tempfile
import time
import unittest

ROOT=Path(__file__).resolve().parents[1]
BINARY=ROOT/'target/debug/amber-shell'


class BootstrapTests(unittest.TestCase):
    def setup_directory(self,directory):
        root=Path(directory)
        for name in ['.zshenv','.zprofile','.zlogin']:
            (root/name).write_text('print -r -- '+shlex.quote(name)+' >> '+shlex.quote(str(root/'startup-count'))+'\n')
        (root/'.zshrc').write_text("HISTFILE="+shlex.quote(str(root/'history'))+"\nSAVEHIST=0\nPS1='BOOT_READY> '\n"
            'print -r -- "$options[login] $SHLVL" > '+shlex.quote(str(root/'login-mode'))+'\n'
            'print -rn -- "$AMBER_SHELL_CAPTURE_SOCKET" > '+shlex.quote(str(root/'endpoint'))+'\n')
        subprocess.run(['sh',str(ROOT/'scripts/install.sh'),'--prefix',str(root/'prefix'),'--zshrc',str(root/'.zshrc'),
            '--zshenv',str(root/'.zshenv'),'--binary',str(BINARY)],check=True,capture_output=True)
        env={**os.environ,'ZDOTDIR':str(root),'SHLVL':'0','SHELL':'/bin/zsh','TERM':'xterm-256color'}
        for key in ['AMBER_SHELL_CAPTURE_SOCKET','AMBER_SHELL_CAPTURE_OWNER','AMBER_SHELL_BIN']:env.pop(key,None)
        return root,env

    def run_tty(self,arguments,env,ready=None):
        pid,fd=pty.fork()
        if pid==0:os.execve('/bin/zsh',['zsh']+arguments,env)
        output=b'';status=None
        try:
            end=time.monotonic()+8
            sent=False
            while time.monotonic()<end:
                if select.select([fd],[],[],.02)[0]:
                    try:output+=os.read(fd,65536)
                    except OSError:pass
                if ready and not sent and ready in output:os.write(fd,b'exit\r');sent=True
                done,result=os.waitpid(pid,os.WNOHANG)
                if done:status=os.waitstatus_to_exitcode(result);break
            self.assertIsNotNone(status,'isolated shell did not exit')
            self.assertEqual(status,0,output[-2000:])
            return output
        finally:
            if status is None:
                os.kill(pid,signal.SIGHUP)
                end=time.monotonic()+3
                while time.monotonic()<end:
                    done,result=os.waitpid(pid,os.WNOHANG)
                    if done:break
                    if select.select([fd],[],[],.02)[0]:
                        try:os.read(fd,65536)
                        except OSError:pass
                else:os.kill(pid,signal.SIGKILL);os.waitpid(pid,0)
            os.close(fd)

    def test_login_startup_once_correct_prefix_and_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root,env=self.setup_directory(directory)
            self.run_tty(['-il'],env,b'BOOT_READY>')
            self.assertEqual((root/'startup-count').read_text().splitlines(),['.zshenv','.zprofile','.zlogin'])
            self.assertEqual((root/'login-mode').read_text().strip(),'on 1')
            endpoint=(root/'endpoint').read_text()
            self.assertTrue(endpoint)
            self.assertFalse(Path(endpoint).exists())
            self.assertEqual((root/'.zshenv').stat().st_mode & 0o777,0o600)

    def test_noninteractive_and_interactive_script_invocations_are_not_wrapped(self):
        with tempfile.TemporaryDirectory() as directory:
            root,env=self.setup_directory(directory)
            result=subprocess.run(['/bin/zsh','-c','print -r -- ${AMBER_SHELL_CAPTURE_OWNER:-OFF}'],env=env,capture_output=True,text=True,check=True)
            self.assertEqual(result.stdout.strip(),'OFF')
            self.assertFalse((root/'endpoint').exists())
            self.run_tty(['-ic','print -r -- ${AMBER_SHELL_CAPTURE_OWNER:-OFF}'],env)
            self.assertEqual((root/'endpoint').read_text(),'')
            script=root/'script.zsh';script.write_text('print -r -- ${AMBER_SHELL_CAPTURE_OWNER:-OFF}\n')
            self.assertIn(b'OFF',self.run_tty(['-i',str(script)],env))

    def test_capture_can_be_disabled_without_affecting_login(self):
        with tempfile.TemporaryDirectory() as directory:
            root,env=self.setup_directory(directory)
            env['AMBER_SHELL_CAPTURE']='0'
            self.run_tty(['-il'],env,b'BOOT_READY>')
            self.assertEqual((root/'endpoint').read_text(),'')
            self.assertEqual((root/'login-mode').read_text().strip(),'on 1')
            self.assertEqual((root/'startup-count').read_text().splitlines(),['.zshenv','.zprofile','.zlogin'])

    def test_capture_setup_failure_falls_back_to_normal_shell(self):
        with tempfile.TemporaryDirectory() as directory:
            root,env=self.setup_directory(directory)
            # A legal filesystem directory whose Unix socket path exceeds sun_path.
            long_directory=root/('long-tmp-'+('x'*90))
            long_directory.mkdir()
            env['TMPDIR']=str(long_directory)
            output=self.run_tty(['-il'],env,b'BOOT_READY>')
            self.assertIn(b'normale Shell startet',output)
            self.assertEqual((root/'endpoint').read_text(),'')
            self.assertEqual((root/'startup-count').read_text().splitlines(),['.zshenv','.zprofile','.zlogin'])


if __name__=='__main__':unittest.main(verbosity=2)
