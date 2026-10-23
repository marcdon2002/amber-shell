#!/usr/bin/env python3
"""Integrated login with the real TUI and synthetic Codex/browser processes only."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zle_binary_e2e as h


def fake_programs(root):
    codex = root / 'codex'
    codex.write_text('''#!/usr/bin/env python3
import json,os,sys,threading,time
from pathlib import Path
root=Path(__file__).parent
if 'mcp' in sys.argv: print('[]');sys.exit(0)
(root/'pid').write_text(str(os.getpid()))
home=Path(os.environ['CODEX_HOME']);auth=home/'auth.json'
lock=threading.Lock()
def emit(x):
 with lock: print(json.dumps(x),flush=True)
def finish():
 deadline=time.monotonic()+20
 while not (root/'finish').exists() and time.monotonic()<deadline:time.sleep(.01)
 if (root/'mode').read_text()=='error':
  emit({'method':'account/login/completed','params':{'loginId':'login-test','success':False,'error':'synthetic failure'}});return
 auth.write_text('{"synthetic_login":true}');auth.chmod(0o600)
 emit({'method':'account/login/completed','params':{'loginId':'login-test','success':True,'error':None}})
for line in sys.stdin:
 d=json.loads(line);m=d['method'];i=d.get('id');p=d.get('params',{})
 with (root/'rpc.jsonl').open('a') as f:f.write(json.dumps({'method':m})+'\\n')
 if m=='initialized':continue
 result={}
 if m=='account/read': result={'account':{'type':'chatgpt'} if auth.exists() else None,'requiresOpenaiAuth':True}
 if m=='config/read':result={'config':{'mcp_servers':{}}}
 if m=='account/login/start':
  assert p['type']=='chatgpt'
  emit({'id':i,'result':{'type':'chatgpt','loginId':'login-test','authUrl':'https://auth.openai.com/authorize?synthetic-private-state=123'}})
  threading.Thread(target=finish,daemon=True).start();continue
 if m=='model/list':result={'data':[{'model':'gpt-6-luna','displayName':'GPT-6 Luna','hidden':False}],'nextCursor':None}
 if m=='thread/start':
  assert auth.exists(),'no model before login'
  result={'thread':{'id':'thread-test'},'model':'gpt-6-luna','modelProvider':'openai','serviceTier':'default'}
 if m=='turn/start':
  assert auth.exists(),'no model before login'
  emit({'id':i,'result':{'turn':{'id':'turn-test'}}})
  text=json.dumps({'explanation':'Lists files.','command':'ls'})
  emit({'method':'item/agentMessage/delta','params':{'threadId':'thread-test','turnId':'turn-test','delta':text}})
  emit({'method':'item/completed','params':{'threadId':'thread-test','turnId':'turn-test','item':{'type':'agentMessage','phase':'final_answer','text':text}}})
  emit({'method':'turn/completed','params':{'threadId':'thread-test','turn':{'id':'turn-test','status':'completed'}}});continue
 emit({'id':i,'result':result})
''')
    opener = root / 'browser'
    opener.write_text('''#!/usr/bin/env python3
from pathlib import Path
import sys
root=Path(__file__).parent
(root/'opened').write_text(sys.argv[1])
''')
    for path in [codex, opener]:path.chmod(0o755)
    (root/'mode').write_text('success')
    return codex, opener


class OnboardingTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='amber-onboarding-')
        self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)
        codex,opener=fake_programs(self.root)
        with patch.dict(os.environ,{'AMBER_BROWSER_BIN':str(opener)}):
            self.s=h.ShellSession(simulate=False,codex_program=codex,signed_in=False)
        self.addCleanup(self.s.close)

    def open_login(self):
        self.s.send(b'\x18\x01')
        self.s.until(lambda:'Log in with ChatGPT' in self.s.text(),label='integrated login offer')

    def wait_browser(self):
        self.s.until(lambda:(self.root/'opened').exists(),label='synthetic browser opened')
        self.s.until(lambda:'browser' in self.s.text().lower() and 'Starting' not in self.s.text(),label='browser wait state')

    def leave_login(self,key=b'\x1b'):
        self.s.send(key)
        self.s.until(lambda:'Log in with ChatGPT' not in self.s.text(),label='login dialog closed')
        self.s.drain(.1)

    def test_first_open_is_idle_and_cancel_restores_original_input(self):
        original="printf '%s' 'ä é 😀'"
        self.s.seed(original,4)
        self.open_login()
        self.assertFalse((self.root/'rpc.jsonl').exists())
        self.assertFalse((self.root/'opened').exists())
        self.s.drain(.1);before=len(self.s.output);self.s.drain(.5)
        self.assertEqual(len(self.s.output),before,'idle login must not animate or poll Codex')
        self.s.send(b'\x19\x0f\x0b\t')
        self.s.drain(.1)
        self.assertIn('Log in with ChatGPT',self.s.text())
        self.leave_login()
        self.assertEqual(self.s.capture(),(original.encode(),4))
        self.s.assert_modes()

    def test_browser_completion_continues_same_dialog_and_persists_login(self):
        self.open_login();self.s.send(b'\r');self.wait_browser()
        self.assertNotIn('synthetic-private-state',self.s.text())
        self.assertNotIn(b'https://auth.openai.com',self.s.output)
        (self.root/'finish').touch()
        self.s.until(lambda:'What would you like to do?' in self.s.text(),label='same-dialog chat after login')
        calls=[json.loads(line)['method'] for line in (self.root/'rpc.jsonl').read_text().splitlines()]
        self.assertNotIn('thread/start',calls)
        self.assertNotIn('turn/start',calls)
        self.s.send('How do I list files?\r')
        self.s.until(lambda:'> ls' in self.s.text(),label='answer after login')
        self.assertEqual(self.s.take(),(b'ls',2))
        self.s.assert_modes()
        self.s.send(b'\x15\x18\x01')
        self.s.until(lambda:'What would you like to do?' in self.s.text(),label='saved login skips onboarding')
        self.s.send(b'\x1b');self.s.drain(.2)
        calls=[json.loads(line)['method'] for line in (self.root/'rpc.jsonl').read_text().splitlines()]
        self.assertEqual(calls.count('account/login/start'),1)

    def test_cancel_waiting_reaps_login_process_and_handles_resize(self):
        self.open_login();self.s.send(b'\r');self.wait_browser()
        for width,height in [(40,14),(24,9),(100,30)]:self.s.resize(width,height);self.s.drain(.15)
        pid=int((self.root/'pid').read_text())
        self.leave_login(b'\x03')
        self.s.assert_modes()
        with self.assertRaises(ProcessLookupError):os.kill(pid,0)
        self.assertFalse((self.s.config_directory/'amber-shell/codex/auth.json').exists())

    def test_failed_login_can_retry_in_the_same_dialog(self):
        (self.root/'mode').write_text('error')
        self.open_login();self.s.send(b'\r');self.wait_browser();(self.root/'finish').touch()
        self.s.until(lambda:"didn't finish" in self.s.text().lower(),label='visible login error')
        (self.root/'mode').write_text('success')
        self.s.send(b'\r')
        self.s.until(lambda:'What would you like to do?' in self.s.text(),label='retry succeeds without reopening')
        self.s.send(b'\x1b');self.s.drain(.2);self.s.assert_modes()


if __name__=='__main__':unittest.main(verbosity=2)
