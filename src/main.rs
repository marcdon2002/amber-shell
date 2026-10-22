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
            .ok_or_else(|| format!("Wert für {key} fehlt")),
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
            eprintln!("amber-shell: Interner Fehler; Terminal wiederhergestellt.");
            1
        }
    };
    std::process::exit(code);
}
fn entry() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
            "amber-shell — kompakter Assistent für zsh / Apple Terminal\n\nOhne Argument: Inline-Dialog mit direkter Frageeingabe\n  --simulate                 Synthetische Antworten, kein Netzwerk\n  --history-file PFAD         NUL-getrennte Shell-Befehle (max. 32 KiB)\n  --capture-socket PFAD       Begrenzte Ausgaben der aktuellen Sitzung\n  --result-file PFAD          Originalbefehl + NUL, nur bei Übernahme\n  session -- /bin/zsh -i      Shell mit Ausgabeerfassung starten\n  login                      Offizielles codex login\n  doctor                     Codex-Version und offizieller Loginstatus\n  ask --question TEXT         Nichtinteraktive Diagnose, JSON-Antwort\n      --model ID              Explizites Modell für diese Diagnose\n      --fast                  Unterstützten Fast-Tier verwenden\n      --context-file PFAD     Bewusst gewählter Kontext (max. 48 KiB)\n      --conversation-file P   JSON-Paare aus Fragen/Antworten\n\nTasten: Ctrl-X Ctrl-A öffnet ZLE-Widget; Enter fragt; Ctrl-Y übernimmt;\nEsc/Ctrl-C brechen ab; Tab und Pfeile lesen/wählen; Ctrl-O wählt Modell/Fast; Ctrl-K bearbeitet den Kontext.\nAusführen erst nach Rückkehr mit normalem Enter in der Shell."
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
        let question = option(&args, "--question")?.ok_or("--question erforderlich")?;
        if question.len() > 8192 || question.trim().is_empty() {
            return Err("Frage leer oder größer als 8 KiB".into());
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
                return Err("Zeitlimit für Modellantwort erreicht".into());
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
                Err(_) => return Err("Provider beendet".into()),
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
        app.status = format!("Ausgabenkontext: {error}");
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
        Err(error) => app.status = format!("Lokaler Verlauf: {error}"),
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
                let parent = path.parent().ok_or("Ungültiger Ergebnispfad")?;
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
        return Err("Kontext muss reguläre Datei sein".into());
    }
    let mut bytes = vec![];
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Eingabedatei überschreitet Größenlimit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Eingabedatei ist kein UTF-8".into())
}
