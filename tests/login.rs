use amber_shell::{
    auth,
    provider::{LoginEvent, start_login_with_program},
};
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

// The auth API intentionally selects Amber's profile from the environment.
static ENV: Mutex<()> = Mutex::new(());
struct Fixture {
    _lock: MutexGuard<'static, ()>,
    _directory: tempfile::TempDir,
    old_config: Option<OsString>,
    root: PathBuf,
    program: PathBuf,
    opener: PathBuf,
}
impl Fixture {
    fn new(mode: &str, url: &str, opener_mode: &str) -> Self {
        let lock = ENV.lock().unwrap_or_else(|poison| poison.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let config = root.join("config");
        let old_config = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &config) };
        let program = root.join("codex");
        let url_json = serde_json::to_string(url).unwrap();
        let script = format!(
            r###"#!/usr/bin/env python3
import json, os, sys, time
mode={mode:?}
url=json.loads({url_json:?})
root=os.path.dirname(__file__)
home=os.path.join(root,'config','amber-shell','codex')
assert os.environ['CODEX_HOME']==home
assert 'cli_auth_credentials_store="file"' in sys.argv
assert 'forced_login_method="chatgpt"' in sys.argv
assert not any(os.environ.get(k) for k in ['OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN','OPENAI_BASE_URL'])
if 'mcp' in sys.argv:
 print(json.dumps([{{'name':'test-server','enabled':True}}])); sys.exit(0)
args=' '.join(sys.argv)
for option in ['mcp_servers.test-server.enabled=false','features.shell_tool=false','features.apps=false','features.plugins=false','features.hooks=false','web_search="disabled"','notify=[]','analytics.enabled=false']:
 assert option in args, option
open(os.path.join(root,'server-pid'),'w').write(str(os.getpid()))
def emit(value): print(json.dumps(value),flush=True)
def completed(login_id='safe-login',success=True):
 emit({{'method':'account/login/completed','params':{{'loginId':login_id,'success':success,'error':'sensitive '+url if not success else None}}}})
for line in sys.stdin:
 d=json.loads(line); method=d['method']; ident=d.get('id')
 with open(os.path.join(root,'methods'),'a') as f: f.write(method+'\n')
 assert method not in ['thread/start','turn/start']
 if method=='initialized': continue
 result={{}}
 if method=='config/read': result={{'config':{{'mcp_servers':{{'test-server':{{'enabled':False}}}}}}}}
 if method=='account/login/start':
  assert d['params']=={{'type':'chatgpt','useHostedLoginSuccessPage':True,'appBrand':'chatgpt'}}
  if mode=='rpc-error':
   emit({{'id':ident,'error':{{'code':-1,'message':url}}}}); continue
  if mode=='early': completed()
  result={{'type':'chatgpt','loginId':'safe-login','authUrl':url}}
 if method=='account/read': result={{'account':None if mode=='account-required' else {{'type':'apiKey' if mode=='api-key' else 'chatgpt'}},'requiresOpenaiAuth':True}}
 emit({{'id':ident,'result':result}})
 if method=='account/login/start':
  if mode in ['wait','rpc-error']: continue
  if mode=='wrong-id': completed('other-login'); continue
  if mode=='failed': completed(success=False); continue
  if mode not in ['unsafe-url','no-file']:
   with open(os.path.join(home,'auth.json'),'w') as f: f.write('opaque credentials, intentionally not JSON')
   os.chmod(os.path.join(home,'auth.json'),0o600)
  if mode!='early': completed()
"###
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let opener = root.join("open");
        fs::write(
            &opener,
            format!(
                r###"#!/usr/bin/env python3
import json,os,sys,time
root=os.path.dirname(__file__)
assert len(sys.argv)==2
open(os.path.join(root,'opened'),'w').write(json.dumps(sys.argv[1:]))
open(os.path.join(root,'opener-pid'),'w').write(str(os.getpid()))
mode={opener_mode:?}
if mode=='hang': time.sleep(60)
if mode=='fail': sys.exit(1)
"###
            ),
        )
        .unwrap();
        fs::set_permissions(&opener, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _lock: lock,
            _directory: directory,
            old_config,
            root,
            program,
            opener,
        }
    }
    fn auth_file(&self) -> PathBuf {
        self.root.join("config/amber-shell/codex/auth.json")
    }
    fn receive(&self) -> Vec<LoginEvent> {
        let handle = start_login_with_program(&self.program, &self.opener);
        let mut events = Vec::new();
        loop {
            let event = handle
                .events
                .recv_timeout(Duration::from_secs(8))
                .expect("bounded login result");
            let finished = !matches!(event, LoginEvent::Waiting);
            events.push(event);
            if finished {
                break;
            }
        }
        drop(handle);
        events
    }
    fn pid(&self, name: &str) -> i32 {
        let path = self.root.join(name);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        fs::read_to_string(path)
            .expect("child started")
            .parse()
            .unwrap()
    }
    fn assert_no_inference(&self) {
        let methods = fs::read_to_string(self.root.join("methods")).unwrap();
        assert!(!methods.contains("thread/start") && !methods.contains("turn/start"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            match &self.old_config {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }
}
const URL: &str = "https://auth.openai.com/oauth/authorize?secret=never-display-this";

#[test]
fn browser_login_persists_the_private_amber_profile_without_inference() {
    let fixture = Fixture::new("normal", URL, "ok");
    assert!(!auth::has_credentials().unwrap());
    assert!(
        !fixture.root.join("config").exists(),
        "presence checks must not create a profile"
    );
    let events = fixture.receive();
    assert!(
        matches!(
            events.as_slice(),
            [LoginEvent::Waiting, LoginEvent::Complete]
        ),
        "{events:?}"
    );
    assert!(auth::has_credentials().unwrap());
    assert_eq!(
        fs::metadata(fixture.auth_file()).unwrap().mode() & 0o777,
        0o600
    );
    let opened: Vec<String> =
        serde_json::from_str(&fs::read_to_string(fixture.root.join("opened")).unwrap()).unwrap();
    assert_eq!(opened, [URL]);
    fixture.assert_no_inference();
    assert!(!format!("{events:?}").contains(URL));
}

#[test]
fn login_completion_arriving_before_acknowledgement_is_preserved() {
    let fixture = Fixture::new("early", URL, "ok");
    assert!(matches!(
        fixture.receive().as_slice(),
        [LoginEvent::Waiting, LoginEvent::Complete]
    ));
}

#[test]
fn trusted_chatgpt_url_is_passed_to_the_launcher_as_one_literal_argument() {
    let url = "https://chatgpt.com/auth/login?state=literal;$(touch%20unexpected)";
    let fixture = Fixture::new("normal", url, "ok");
    assert!(matches!(
        fixture.receive().as_slice(),
        [LoginEvent::Waiting, LoginEvent::Complete]
    ));
    let opened: Vec<String> =
        serde_json::from_str(&fs::read_to_string(fixture.root.join("opened")).unwrap()).unwrap();
    assert_eq!(opened, [url]);
}

#[test]
fn failed_login_and_unconfirmed_or_unpersisted_accounts_cannot_complete() {
    for mode in [
        "failed",
        "account-required",
        "api-key",
        "no-file",
        "rpc-error",
    ] {
        let fixture = Fixture::new(mode, URL, "ok");
        let events = fixture.receive();
        assert!(
            matches!(events.last(), Some(LoginEvent::Error(_))),
            "{mode}: {events:?}"
        );
        assert!(
            !format!("{events:?}").contains("secret"),
            "sensitive server error escaped"
        );
        fixture.assert_no_inference();
    }
}

#[test]
fn unsafe_browser_urls_are_never_opened_or_exposed_in_events() {
    let urls = [
        "http://auth.openai.com/path".to_string(),
        "https://auth.openai.com.evil/path".into(),
        "https://auth.openai.com@evil/path".into(),
        "https://chatgpt.com:444/path".into(),
        "https://chatgpt.com/path\nsecret".into(),
        "https://chatgpt.com/path\u{1b}[2J".into(),
        "https://evil/path".into(),
        format!("https://auth.openai.com/{}", "x".repeat(8192)),
    ];
    for url in urls {
        let fixture = Fixture::new("unsafe-url", &url, "ok");
        let events = fixture.receive();
        assert!(
            matches!(events.as_slice(), [LoginEvent::Error(_)]),
            "{events:?}"
        );
        assert!(!fixture.root.join("opened").exists());
        assert!(!format!("{events:?}").contains(&url));
    }
}

#[test]
fn unrelated_completion_is_ignored_and_cancellation_reaps_the_callback_server() {
    let fixture = Fixture::new("wrong-id", URL, "ok");
    let handle = start_login_with_program(&fixture.program, &fixture.opener);
    assert!(matches!(
        handle.events.recv_timeout(Duration::from_secs(3)).unwrap(),
        LoginEvent::Waiting
    ));
    assert!(
        handle
            .events
            .recv_timeout(Duration::from_millis(150))
            .is_err()
    );
    let pid = fixture.pid("server-pid");
    let started = Instant::now();
    drop(handle);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "callback process was not reaped"
    );
}

#[test]
fn cancelling_a_stuck_browser_launcher_reaps_both_owned_children() {
    let fixture = Fixture::new("wait", URL, "hang");
    let handle = start_login_with_program(&fixture.program, &fixture.opener);
    let browser_pid = fixture.pid("opener-pid");
    let server_pid = fixture.pid("server-pid");
    let started = Instant::now();
    drop(handle);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(unsafe { libc::kill(browser_pid, 0) }, -1);
    assert_eq!(unsafe { libc::kill(server_pid, 0) }, -1);
}

#[test]
fn failed_browser_launcher_never_reports_waiting_or_completion() {
    let fixture = Fixture::new("wait", URL, "fail");
    assert!(matches!(
        fixture.receive().as_slice(),
        [LoginEvent::Error(_)]
    ));
}

#[test]
fn stuck_browser_launcher_times_out_and_reaps_both_owned_children() {
    let fixture = Fixture::new("wait", URL, "hang");
    let started = Instant::now();
    assert!(matches!(
        fixture.receive().as_slice(),
        [LoginEvent::Error(_)]
    ));
    assert!(started.elapsed() < Duration::from_secs(7));
    for name in ["server-pid", "opener-pid"] {
        assert_eq!(unsafe { libc::kill(fixture.pid(name), 0) }, -1);
    }
}

#[test]
fn credential_presence_validates_metadata_without_parsing_the_contents() {
    let fixture = Fixture::new("normal", URL, "ok");
    assert!(!auth::has_credentials().unwrap());
    drop(auth::command(&fixture.program).unwrap());
    fs::write(fixture.auth_file(), "opaque and invalid JSON").unwrap();
    fs::set_permissions(fixture.auth_file(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(auth::has_credentials().unwrap());
    fs::set_permissions(fixture.auth_file(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(auth::has_credentials().is_err());
    fs::remove_file(fixture.auth_file()).unwrap();
    std::os::unix::fs::symlink("/dev/null", fixture.auth_file()).unwrap();
    assert!(auth::has_credentials().is_err());
    fs::remove_file(fixture.auth_file()).unwrap();
    fs::write(fixture.auth_file(), "private credential").unwrap();
    fs::set_permissions(fixture.auth_file(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(fixture.auth_file(), fixture.root.join("credential-link")).unwrap();
    assert!(auth::has_credentials().is_err());
}
