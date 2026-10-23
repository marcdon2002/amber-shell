use amber_shell::{
    model::{App, Reply, Turn, validate_command},
    provider::{self, ProviderEvent, ProviderRequest},
    terminal,
};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

fn option(args: &[String], key: &str) -> Result<Option<String>, String> {
    match args.iter().position(|a| a == key) {
        Some(i) => args
            .get(i + 1)
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("Missing value for {key}")),
        None => Ok(None),
    }
}
fn main() {
    let result = std::panic::catch_unwind(entry);
    let code = match result {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            eprintln!("amber-shell: {e}");
            1
        }
        Err(_) => {
            eprintln!("amber-shell: Internal error; terminal restored.");
            1
        }
    };
    std::process::exit(code);
}
fn entry() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--version" || s == "-V") {
        println!("amber-shell {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if args.first().is_some_and(|s| s == "session") {
        return match amber_shell::session::run(&args[1..]) {
            Ok(code) => Ok(code),
            Err(error) => amber_shell::session::fallback(&args[1..], &error),
        };
    }
    let program =
        std::env::var("AMBER_CODEX_BIN").unwrap_or_else(|_| "/opt/homebrew/bin/codex".into());
    if args.iter().any(|s| s == "--help" || s == "-h") {
        println!(
            "amber-shell — compact assistant for zsh / Apple Terminal\n\nNo arguments: Inline chat with direct question input\n  --simulate                 Simulated replies, no network\n  --history-file PATH         NUL-separated shell commands (max. 32 KiB)\n  --capture-socket PATH       Bounded output from the current session\n  --result-file PATH          Original command + NUL, only on acceptance\n  session -- /bin/zsh -i      Start a shell with output capture\n  login                      Official codex login\n  doctor                     Codex version and official login status\n  ask --question TEXT         Noninteractive diagnostic, JSON reply\n      --model ID              Explicit model for this diagnostic\n      --fast                  Use the supported Fast tier\n      --context-file PATH     Explicitly selected context (max. 48 KiB)\n      --conversation-file P   JSON pairs of questions/replies\n\nKeys: Ctrl-X Ctrl-A opens the ZLE widget; Enter asks; Ctrl-Y accepts;\nEsc/Ctrl-C cancel; Tab and arrows browse/select; Ctrl-O selects model/Fast; Ctrl-K edits context.\nPress Enter at the normal shell prompt to execute the accepted command."
        );
        return Ok(0);
    }
    if args.first().is_some_and(|s| s == "login" || s == "doctor") {
        let mut cmd = std::process::Command::new(program);
        if args[0] == "doctor" {
            let status = cmd.arg("--version").status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Ok(1);
            }
            return Ok(std::process::Command::new(
                std::env::var("AMBER_CODEX_BIN")
                    .unwrap_or_else(|_| "/opt/homebrew/bin/codex".into()),
            )
            .args(["login", "status"])
            .status()
            .map_err(|e| e.to_string())?
            .code()
            .unwrap_or(1));
        }
        return Ok(cmd
            .arg("login")
            .status()
            .map_err(|e| e.to_string())?
            .code()
            .unwrap_or(1));
    }
    let simulated = args.iter().any(|s| s == "--simulate");
    if args.first().is_some_and(|s| s == "ask") {
        let question = option(&args, "--question")?.ok_or("--question is required")?;
        if question.len() > 8192 || question.trim().is_empty() {
            return Err("Question is empty or exceeds 8 KiB".into());
        }
        let context = option(&args, "--context-file")?
            .map(|p| read_bounded(&p, 48 * 1024))
            .transpose()?
            .unwrap_or_default();
        let conversation = option(&args, "--conversation-file")?
            .map(|p| read_bounded(&p, 6 * 1024 * 1024))
            .transpose()?
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        let handle = provider::start(
            ProviderRequest {
                fast: args.iter().any(|s| s == "--fast"),
                model: match option(&args, "--model")? {
                    Some(id) => id,
                    None => amber_shell::settings::load_model()?,
                },
                question,
                context,
                conversation,
            },
            simulated,
        );
        let until = Instant::now() + Duration::from_secs(180);
        loop {
            if Instant::now() > until {
                return Err("Model response timed out".into());
            }
            match handle.events.recv_timeout(Duration::from_millis(200)) {
                Ok(ProviderEvent::Complete(reply)) => {
                    validate_command(&reply.command)?;
                    println!(
                        "{}",
                        serde_json::to_string(&reply).map_err(|e| e.to_string())?
                    );
                    return Ok(0);
                }
                Ok(ProviderEvent::Error(e)) => return Err(e),
                Ok(ProviderEvent::Delta(_)) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("Provider disconnected".into()),
            }
        }
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let history = option(&args, "--history-file")?
        .map(|p| read_bounded(&p, 32 * 1024))
        .transpose()?
        .map(|s| {
            s.split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (outputs, capture_error) = match option(&args, "--capture-socket")? {
        Some(path) => match amber_shell::capture::query(std::path::Path::new(&path)) {
            Ok(outputs) => (outputs, None),
            Err(error) => (vec![], Some(error)),
        },
        None => (vec![], None),
    };
    let mut app = App::from_shell_context_with_outputs(&cwd, &history, &outputs, simulated);
    if let Some(error) = capture_error {
        app.status = format!("Output context: {error}");
    }
    app.history_cwd = Some(cwd.clone());
    match amber_shell::history::load_for_mode(&cwd, simulated) {
        Ok(entries) => {
            app.past_turns = entries
                .into_iter()
                .map(|e| Turn {
                    question: e.question,
                    stream: String::new(),
                    reply: Some(Reply {
                        explanation: e.explanation,
                        command: e.command,
                    }),
                    error: None,
                })
                .collect()
        }
        Err(error) => app.status = format!("Local history: {error}"),
    }
    let settings = amber_shell::settings::load()?;
    app.model_id = settings.model;
    app.fast = settings.fast;
    app.set_context_choice(settings.context.choice());
    match terminal::run(app, simulated)? {
        Some(command) => {
            validate_command(&command)?;
            if let Some(path) = option(&args, "--result-file")? {
                let path = PathBuf::from(path);
                let parent = path.parent().ok_or("Invalid result path")?;
                let mut temp =
                    tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
                temp.write_all(command.as_bytes())
                    .and_then(|_| temp.write_all(&[0]))
                    .map_err(|e| e.to_string())?;
                temp.flush().map_err(|e| e.to_string())?;
                temp.persist_noclobber(path).map_err(|e| e.to_string())?;
            } else {
                println!("\r\n{command}");
            }
            Ok(0)
        }
        None => Ok(2),
    }
}
fn read_bounded(path: &str, limit: usize) -> Result<String, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Context must be a regular file".into());
    }
    let mut bytes = vec![];
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Input file exceeds the size limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Input file is not UTF-8".into())
}
