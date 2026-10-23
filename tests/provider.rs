use amber_shell::provider::{
    ProviderEvent, ProviderRequest, discover_models_with_program, start_with_program,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

fn request() -> ProviderRequest {
    ProviderRequest {
        fast: false,
        model: "gpt-6-luna".into(),
        question: "Print amber".into(),
        context: "Synthetic context".into(),
        conversation: vec![("Before".into(), "Earlier reply".into())],
    }
}

fn fake(mode: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex");
    let script = format!(
        r###"#!/usr/bin/env python3
import sys,json,time,os,fcntl,termios,array
mode={mode:?}
if 'mcp' in sys.argv:
 print(json.dumps([{{'name':'private-server','enabled':True}}])); sys.exit(0)
args=' '.join(sys.argv)
assert 'mcp_servers.private-server.enabled=false' in args, args
for key in ['features.shell_tool=false','features.apps=false','features.plugins=false','features.hooks=false','web_search="disabled"','forced_login_method="chatgpt"','notify=[]','analytics.enabled=false','otel.exporter="none"','otel.trace_exporter="none"']:
 assert key in args, key
def emit(o): print(json.dumps(o),flush=True)
pidfile=os.path.join(os.path.dirname(__file__),'pid')
with open(pidfile+'.tmp','w') as f: f.write(str(os.getpid()))
os.replace(pidfile+'.tmp',pidfile)
for line in sys.stdin:
 d=json.loads(line); method=d['method']; i=d.get('id'); p=d.get('params',{{}})
 if method=='initialized': continue
 result={{}}
 if method=='account/read': result={{'account':{{'type':'apiKey' if mode=='apikey' else 'chatgpt'}},'requiresOpenaiAuth':True}}
 if method=='model/list' and mode=='catalog-hang': time.sleep(60)
 if method=='model/list': result={{'data':[{{'id':'available-test-model','model':'available-test-model','displayName':'Test Default','hidden':False,'isDefault':True}},{{'id':'gpt-6-luna','model':'gpt-6-luna','displayName':'GPT-6 Luna','serviceTiers':[] if mode=='no-fast' else [{{'id':'priority','name':'Fast','description':'Faster'}}],'hidden':False,'isDefault':False}}],'nextCursor':None}}
 if method=='model/list' and mode=='pagination':
  if p.get('cursor') is None: result['data']=result['data'][:1]; result['nextCursor']='page-two'
  else: assert p['cursor']=='page-two'; result['data']=result['data'][1:]
 if method=='thread/start': open(os.path.join(os.path.dirname(__file__),'inference'),'w').write('started')
 if method=='config/read': result={{'config':{{'mcp_servers':{{'private-server':{{'enabled':mode=='unsafe-config'}}}}}}}}
 if method=='thread/start':
  assert 'simplest' in p['baseInstructions'] and 'ls' in p['baseInstructions']
  assert 'Answer in English unless the user explicitly requests another language.' in p['developerInstructions']
  assert p['ephemeral'] is True and p['environments']==[] and p['model']=='gpt-6-luna'
  assert p['modelProvider']=='openai' and p['sandbox']=='read-only'
  assert p['serviceTier']==('priority' if mode in ['fast','wrong-tier'] else 'default')
  result={{'thread':{{'id':'thread-test'}},'model':'available-test-model' if mode=='wrong-model' else 'gpt-6-luna','modelProvider':'openai','serviceTier':'default' if mode=='wrong-tier' else p['serviceTier']}}
 if method=='turn/start':
  assert p['threadId']=='thread-test' and p['environments']==[]
  assert p['serviceTierForTurn']==('priority' if mode=='fast' else 'default')
  assert p['effort']=='low'
  assert p['sandboxPolicy']=={{'type':'readOnly','networkAccess':False}}
  assert 'Earlier reply' in p['input'][0]['text']
  assert p['outputSchema']['required']==['explanation','command']
  if mode=='eof': sys.exit(0)
  if mode=='hang': time.sleep(60)
  if mode=='error': emit({{'id':i,'error':{{'code':-1,'message':'synthetic error'}}}}); continue
  emit({{'id':i,'result':{{'turn':{{'id':'turn-test'}}}}}})
  if mode=='tool':
   emit({{'id':99,'method':'item/tool/call','params':{{}}}}); continue
  text=json.dumps({{'explanation':'Very short','command':"printf '%s\\n' 'ä';\nprintf done\n"}})
  if mode=='oversize': text='x'*270000
  emit({{'method':'item/agentMessage/delta','params':{{'threadId':'thread-test','turnId':'turn-test','itemId':'answer','delta':text[:20]}}}})
  emit({{'method':'item/agentMessage/delta','params':{{'threadId':'thread-test','turnId':'turn-test','itemId':'answer','delta':text[20:]}}}})
  emit({{'method':'item/completed','params':{{'threadId':'thread-test','turnId':'turn-test','item':{{'type':'agentMessage','id':'answer','text':text,'phase':'final_answer'}}}}}})
  if mode=='missing-completion': sys.exit(0)
  emit({{'method':'turn/completed','params':{{'threadId':'thread-test','turn':{{'id':'turn-test','status':'completed','error':None}}}}}})
  continue
 emit({{'id':i,'result':result}})
 if method=='thread/start' and mode in ['backpressure','serializing']:
  if mode=='backpressure':
   deadline=time.monotonic()+10
   while time.monotonic()<deadline:
    pending=array.array('i',[0]); fcntl.ioctl(sys.stdin.fileno(),termios.FIONREAD,pending,True)
    if pending[0]>=8192: break
    time.sleep(.005)
   else: sys.exit(2)
  open(os.path.join(os.path.dirname(__file__),'ready'),'w').write('ready')
  time.sleep(60)
"###
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    (dir, path)
}

fn finish(mode: &str) -> Result<(String, String), String> {
    finish_with_request(mode, request())
}
fn finish_with_request(mode: &str, req: ProviderRequest) -> Result<(String, String), String> {
    let (_dir, path) = fake(mode);
    let handle = start_with_program(req, false, &path);
    let mut saw_delta = false;
    loop {
        match handle
            .events
            .recv_timeout(Duration::from_secs(8))
            .expect("provider result within timeout")
        {
            ProviderEvent::Delta(_) => saw_delta = true,
            ProviderEvent::Complete(reply) => {
                assert!(saw_delta);
                return Ok((reply.explanation, reply.command));
            }
            ProviderEvent::Error(e) => return Err(e),
        }
    }
}

#[test]
fn complete_reply_keeps_original_multiline_unicode_command() {
    let (explanation, command) = finish("normal").unwrap();
    assert_eq!(explanation, "Very short");
    assert_eq!(command, "printf '%s\\n' 'ä';\nprintf done\n");
}

#[test]
fn api_key_login_cannot_fall_back_to_paid_requests() {
    let e = finish("apikey").unwrap_err();
    assert!(e.contains("ChatGPT"), "{e}");
}

#[test]
fn unexpected_tool_request_is_refused() {
    let e = finish("tool").unwrap_err();
    assert!(e.contains("tool"), "{e}");
}

#[test]
fn incomplete_eof_never_becomes_command() {
    let e = finish("eof").unwrap_err();
    assert!(e.contains("ended") || e.contains("EOF"), "{e}");
}

#[test]
fn final_message_without_turn_completion_cannot_be_accepted() {
    let e = finish("missing-completion").unwrap_err();
    assert!(e.contains("ended") || e.contains("EOF"), "{e}");
}

#[test]
fn inherited_enabled_mcp_configuration_aborts_before_inference() {
    let e = finish("unsafe-config").unwrap_err();
    assert!(e.contains("isolation"), "{e}");
}

#[test]
fn server_error_is_reported_without_completion() {
    let e = finish("error").unwrap_err();
    assert!(e.contains("Codex"), "{e}");
}

#[test]
fn oversized_stream_is_rejected() {
    let e = finish("oversize").unwrap_err();
    assert!(e.contains("limit"), "{e}");
}

#[test]
fn cancellation_ends_waiting_child_promptly() {
    let (_dir, path) = fake("hang");
    let handle = start_with_program(request(), false, &path);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.parent().unwrap().join("pid").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = fs::read_to_string(path.parent().unwrap().join("pid"))
        .expect("server started")
        .parse()
        .unwrap();
    let began = Instant::now();
    drop(handle);
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "cancelled server must be reaped"
    );
}

#[test]
fn cancellation_interrupts_full_stdin_pipe() {
    let mut req = request();
    req.conversation = vec![("Before".into(), "x".repeat(200_000)); 18];
    assert_large_request_cancellation("backpressure", req);
}

#[test]
fn cancellation_interrupts_serializing_maximum_escaped_conversation() {
    let mut req = request();
    req.conversation = vec![("Before".into(), "\n".repeat(256 * 1024)); 20];
    assert_large_request_cancellation("serializing", req);
}

fn assert_large_request_cancellation(mode: &str, req: ProviderRequest) {
    let (_dir, path) = fake(mode);
    let handle = start_with_program(req, false, &path);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.parent().unwrap().join("ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.parent().unwrap().join("ready").exists());
    let pid: i32 = fs::read_to_string(path.parent().unwrap().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        drop(handle);
        tx.send(()).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(1));
    if result.is_err() {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    worker.join().unwrap();
    assert!(
        result.is_ok(),
        "provider drop must interrupt {mode} in less than 1s"
    );
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
#[ignore = "Uses the existing ChatGPT plan for two real, synthetic model requests"]
fn real_codex_context_and_followup() {
    fn answer(req: ProviderRequest) -> String {
        let handle =
            start_with_program(req, false, std::path::Path::new("/opt/homebrew/bin/codex"));
        loop {
            match handle
                .events
                .recv_timeout(Duration::from_secs(150))
                .unwrap()
            {
                ProviderEvent::Delta(_) => {}
                ProviderEvent::Error(error) => panic!("{error}"),
                ProviderEvent::Complete(reply) => {
                    println!(
                        "Synthetic live result: {} | {}",
                        reply.explanation, reply.command
                    );
                    return serde_json::to_string(&reply).unwrap();
                }
            }
        }
    }
    let models = discover_models_with_program(
        std::path::Path::new("/opt/homebrew/bin/codex"),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    println!("Live model catalog: {:?}", models);
    assert!(models.iter().any(|model| model.id == "gpt-6-luna"));
    let q = "Schlage printf vor, das den Projektnamen aus dem Kontext ohne Zeilenumbruch ausgibt. Nicht ausführen.";
    let first = answer(ProviderRequest {
        fast: false,
        model: "gpt-6-luna".into(),
        question: q.into(),
        context: "Synthetic README: Projektname AMBER_PROBE_42".into(),
        conversation: vec![],
    });
    assert!(first.contains("AMBER_PROBE_42"));
    let second=answer(ProviderRequest{fast:false,model:"gpt-6-luna".into(),question:"Ergänze beim vorigen Vorschlag unmittelbar nach dem Projektnamen den Suffix _OK. Nicht ausführen.".into(),context:String::new(),conversation:vec![(q.into(),first)]});
    assert!(second.contains("AMBER_PROBE_42"));
    assert!(second.contains("_OK"));
}

#[test]
fn catalog_is_paginated_without_starting_inference() {
    let (dir, path) = fake("pagination");
    let models = discover_models_with_program(
        &path,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(
        models
            .iter()
            .map(|m| (m.id.as_str(), m.label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("available-test-model", "Test Default"),
            ("gpt-6-luna", "GPT-6 Luna")
        ]
    );
    assert!(!dir.path().join("inference").exists());
}

#[test]
fn missing_selected_model_cannot_silently_use_default() {
    let (dir, path) = fake("normal");
    let mut req = request();
    req.model = "unavailable-model".into();
    let handle = start_with_program(req, false, &path);
    match handle.events.recv_timeout(Duration::from_secs(8)).unwrap() {
        ProviderEvent::Error(e) => assert!(e.contains("unavailable-model"), "{e}"),
        event => panic!("Unavailable model must produce an error: {event:?}"),
    }
    assert!(!dir.path().join("inference").exists());
}

#[test]
fn model_discovery_requires_chatgpt_authentication() {
    let (_dir, path) = fake("apikey");
    let e = discover_models_with_program(
        &path,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(e.contains("ChatGPT"), "{e}");
}

#[test]
fn catalog_cancellation_reaps_its_child_promptly() {
    let (dir, path) = fake("catalog-hang");
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancel.clone();
    let worker = std::thread::spawn(move || discover_models_with_program(&path, flag));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.path().join("pid").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = fs::read_to_string(dir.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let began = Instant::now();
    cancel.store(true, std::sync::atomic::Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn server_model_substitution_is_rejected() {
    let error = finish("wrong-model").unwrap_err();
    assert!(error.contains("model"), "{error}");
}

#[test]
fn fast_is_a_validated_priority_tier_independent_of_reasoning() {
    let mut req = request();
    req.fast = true;
    assert!(finish_with_request("fast", req).is_ok());
}
#[test]
fn unsupported_fast_cannot_start_inference() {
    let mut req = request();
    req.fast = true;
    let (dir, path) = fake("no-fast");
    let handle = start_with_program(req, false, &path);
    match handle.events.recv_timeout(Duration::from_secs(8)).unwrap() {
        ProviderEvent::Error(error) => assert!(error.contains("Fast"), "{error}"),
        other => panic!("Unexpected result: {other:?}"),
    }
    assert!(!dir.path().join("inference").exists());
}
#[test]
fn unconfirmed_fast_tier_cannot_silently_use_standard() {
    let mut req = request();
    req.fast = true;
    let error = finish_with_request("wrong-tier", req).unwrap_err();
    assert!(
        error.contains("Fast") || error.contains("service tier"),
        "{error}"
    );
}
#[test]
fn catalog_capabilities_distinguish_models_with_fast() {
    let (_dir, path) = fake("normal");
    let models = discover_models_with_program(
        &path,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    assert!(!models[0].supports_fast);
    assert!(models[1].supports_fast);
}

#[test]
#[ignore = "Uses existing ChatGPT plan for one harmless Luna Fast service-tier request"]
fn real_codex_fast_service_tier() {
    let handle=start_with_program(ProviderRequest {
        model:"gpt-6-luna".into(),fast:true,
        question:"Schlage printf vor, das exakt AMBER_FAST_PROBE ohne Zeilenumbruch ausgibt. Nicht ausführen.".into(),
        context:String::new(),conversation:vec![],
    },false,std::path::Path::new("/opt/homebrew/bin/codex"));
    loop {
        match handle
            .events
            .recv_timeout(Duration::from_secs(150))
            .unwrap()
        {
            ProviderEvent::Delta(_) => {}
            ProviderEvent::Error(error) => panic!("{error}"),
            ProviderEvent::Complete(reply) => {
                assert!(reply.command.contains("AMBER_FAST_PROBE"));
                println!(
                    "Confirmed Luna priority request: {} | {}",
                    reply.explanation, reply.command
                );
                break;
            }
        }
    }
}

#[test]
fn diagnostic_fast_flag_is_explicit_and_does_not_inherit_saved_fast() {
    for (mode, saved_fast, flag) in [("normal", true, false), ("fast", false, true)] {
        let (dir, path) = fake(mode);
        let config = dir.path().join("config");
        fs::create_dir_all(config.join("amber-shell")).unwrap();
        fs::write(
            config.join("amber-shell/settings.json"),
            serde_json::to_vec(&serde_json::json!({"model":"gpt-6-luna","fast":saved_fast}))
                .unwrap(),
        )
        .unwrap();
        let conversation = dir.path().join("conversation.json");
        fs::write(&conversation, r#"[["Before","Earlier reply"]]"#).unwrap();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_amber-shell"));
        command
            .args(["ask", "--question", "Print amber", "--conversation-file"])
            .arg(&conversation)
            .env("AMBER_CODEX_BIN", &path)
            .env("XDG_CONFIG_HOME", &config);
        if flag {
            command.arg("--fast");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let reply: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(reply["command"].as_str().unwrap().contains("printf"));
    }
}

#[test]
#[ignore = "Uses existing ChatGPT plan for simple-first, explicit detail and shell context checks"]
fn real_codex_simple_first_and_shell_context() {
    fn answer(
        question: &str,
        context: String,
        conversation: Vec<(String, String)>,
    ) -> amber_shell::model::Reply {
        let started = Instant::now();
        let handle = start_with_program(
            ProviderRequest {
                fast: true,
                model: "gpt-6-luna".into(),
                question: question.into(),
                context,
                conversation,
            },
            false,
            std::path::Path::new("/opt/homebrew/bin/codex"),
        );
        loop {
            match handle
                .events
                .recv_timeout(Duration::from_secs(120))
                .unwrap()
            {
                ProviderEvent::Complete(reply) => {
                    println!(
                        "{question} -> {:?} ({:.2}s)",
                        reply,
                        started.elapsed().as_secs_f64()
                    );
                    return reply;
                }
                ProviderEvent::Error(e) => panic!("live request failed: {e}"),
                ProviderEvent::Delta(_) => {}
            }
        }
    }
    let question = "Wie liste ich alle Ordner auf?";
    let basic = answer(question, String::new(), vec![]);
    assert_eq!(basic.command, "ls");
    let detail = answer(
        "Jetzt bitte mit versteckten Einträgen und Details.",
        String::new(),
        vec![(question.into(), serde_json::to_string(&basic).unwrap())],
    );
    assert!(detail.command.starts_with("ls "), "{}", detail.command);
    assert!(
        detail.command.contains('a') && detail.command.contains('l'),
        "{}",
        detail.command
    );
    assert!(!detail.command.contains('|') && !detail.command.contains(';'));
    let dir = tempfile::tempdir().unwrap();
    let command = "printf '%s\\n' 'SHELL_CONTEXT_LIVE_72'";
    let app = amber_shell::model::App::from_shell_context(dir.path(), &[command.into()], false);
    let recalled = answer(
        "Gib meinen zuletzt ausgeführten Shell-Befehl unverändert zurück.",
        app.selected_context().into(),
        vec![],
    );
    assert_eq!(recalled.command, command);
}
