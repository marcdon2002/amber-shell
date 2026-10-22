//! Deterministic snapshots of the actual Ratatui renderer, not native screenshots.
use amber_shell::{
    model::{App, Reply, Turn},
    provider::ModelInfo,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "work/render-frames".into());
    std::fs::create_dir_all(&directory)?;
    for (width, height) in [
        (160, 20),
        (100, 20),
        (40, 20),
        (100, 18),
        (40, 18),
        (160, 14),
        (100, 14),
        (40, 14),
        (40, 10),
        (40, 9),
        (40, 6),
        (24, 14),
        (24, 9),
    ] {
        for state in [
            "context",
            "start",
            "waiting",
            "stream",
            "final",
            "final-wrapped",
            "error",
            "followup",
            "model-picker",
            "fast-picker",
            "context-off",
            "markdown",
            "history",
            "long-answer",
            "waiting-1",
            "waiting-2",
            "waiting-3",
            "stream-1",
            "stream-2",
            "stream-3",
            "ascii-final",
            "no-color",
            "design-selected",
        ] {
            let context = amber_shell::context::Context {
                outputs: vec![amber_shell::capture::CaptureRecord {
                    command: "cargo test".into(),
                    cwd: "/demo/amber-project".into(),
                    status: 101,
                    output: "test command_output ... FAILED\n".into(),
                    truncated: false,
                    omitted_reason: None,
                }],
                cwd: "/demo/amber-project".into(),
                files: vec![amber_shell::context::ContextFile {
                    path: "apps/web/package.json".into(),
                    content: "Manifest-Auszug: {\"scripts\":{\"dev\":\"vite\"}}".into(),
                }],
                history: vec!["cargo test".into()],
                omitted: 0,
            };
            let payloads = [
                context.payload(false, false),
                context.payload(true, false),
                context.payload(false, true),
                context.payload(true, true),
            ];
            let mut app = App::new(std::array::from_fn(|i| payloads[i].text.clone()), false);
            app.context_details = std::array::from_fn(|i| payloads[i].details.clone());
            app.context_counts =
                std::array::from_fn(|i| (payloads[i].files.len(), payloads[i].commands.len()));
            app.context_folder = "amber-project".into();
            if state == "context" {
                app.context_choice = 3;
            }
            if state == "context-off" {
                app.context_choice = 4;
            }
            app.ascii = state == "ascii-final";
            app.ascii_input = state == "ascii-final";
            app.no_color = false;
            app.reviewing = state == "context" || state == "context-off";
            app.models = vec![
                ModelInfo {
                    id: "gpt-6-luna".into(),
                    label: "GPT-6 Luna".into(),
                    supports_fast: true,
                },
                ModelInfo {
                    id: "gpt-6-sol".into(),
                    label: "GPT-6 Sol".into(),
                    supports_fast: false,
                },
                ModelInfo {
                    id: "gpt-6-astra".into(),
                    label: "GPT-6 Astra".into(),
                    supports_fast: true,
                },
            ];
            app.model_picker = state == "model-picker" || state == "fast-picker";
            app.picker_fast = state == "fast-picker";
            app.model_cursor = if state == "fast-picker" { 0 } else { 1 };
            if state.starts_with("waiting")
                || state.starts_with("stream")
                || ["final", "final-wrapped", "error", "ascii-final", "no-color"].contains(&state)
            {
                app.editor.insert("Wie prüfe ich dieses Projekt?");
                app.begin()?;
                if !state.starts_with("waiting") && state != "error" {
                    app.delta("{\"explanation\":\"Führe die Tests des Rust-Projekts aus. Damit prüfst du die bestehenden Funktionen.")?;
                }
                if ["final", "final-wrapped", "ascii-final", "no-color"].contains(&state) {
                    let command = if state == "final-wrapped" {
                        "printf '%s\\n' 'Ein langer kontrollierter Beispieltext zeigt den Zeilenumbruch dieses Befehls auch bei einem Terminal mit einhundert Spalten.'\nprintf '%s\\n' 'Eine zweite Befehlszeile bleibt ebenfalls vollständig hervorgehoben.'"
                    } else {
                        "cargo test"
                    };
                    app.complete(Reply { explanation: "Führe die Tests des Rust-Projekts aus. Damit prüfst du die bestehenden Funktionen.".into(), command: command.into() })?;
                }
                if state == "error" {
                    app.fail("Verbindung unterbrochen. Bitte erneut versuchen.".into());
                }
            }
            if state == "markdown" {
                app.editor.insert("Wie zeige ich die Namen im Ordner?");
                app.begin()?;
                app.complete(Reply {
                    explanation: "Mit `ls` werden **Dateien und Ordner** aufgelistet.".into(),
                    command: "ls".into(),
                })?;
            }
            if state == "design-selected" {
                for (question, explanation, command) in [
                    (
                        "Wie liste ich Dateien und Ordner auf?",
                        "Mit ls siehst du die Einträge im aktuellen Ordner.",
                        "ls",
                    ),
                    (
                        "Auch die versteckten?",
                        "Mit -a werden auch versteckte Einträge angezeigt.",
                        "ls -a",
                    ),
                ] {
                    app.editor.insert(question);
                    app.begin()?;
                    app.complete(Reply {
                        explanation: explanation.into(),
                        command: command.into(),
                    })?;
                }
            }
            if state == "followup" {
                app.editor.insert("Wie prüfe ich dieses Projekt?");
                app.begin()?;
                app.complete(Reply {
                    explanation: "Starte zunächst die vorhandenen Tests.".into(),
                    command: "cargo test".into(),
                })?;
                app.editor
                    .insert("Wie zeige ich dabei auch die Testausgaben?");
                app.begin()?;
                app.delta("{\"explanation\":\"Du kannst die Ausgabe der Tests mit einer zusätzlichen Option einblenden.")?;
            }
            if state == "long-answer" {
                app.editor
                    .insert("Wie prüfe ich das Projekt und zeige die Testausgaben?");
                app.begin()?;
                app.complete(Reply {
                    explanation: "Prüfe zuerst das Projekt mit den vorhandenen Tests.
Die zweite Zeile prüft anschließend die Formatierung.
Du kannst beide Befehle vor der Ausführung bearbeiten.
Die Testausgaben bleiben im normalen Terminal sichtbar."
                        .into(),
                    command: "cargo test -- --nocapture\ncargo fmt --check".into(),
                })?;
            }
            if state == "history" {
                app.shell_commands = vec!["git status --short".into(), "cargo test".into()];
                app.past_turns = vec![Turn {
                    question: "Wie zeige ich offene Ports?".into(),
                    stream: String::new(),
                    error: None,
                    reply: Some(Reply {
                        explanation: "Zeigt offene TCP-Ports ohne DNS-Auflösung.".into(),
                        command: "lsof -nP -iTCP -sTCP:LISTEN".into(),
                    }),
                }];
                app.toggle_history();
            }
            app.indicator_frame = state
                .rsplit('-')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            app.no_color = state == "no-color";
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend)?;
            terminal.draw(|f| ui::render(f, &mut app))?;
            let cells: Vec<_> = terminal.backend().buffer().content.iter().map(|cell| json!({"symbol":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"modifier":format!("{:?}",cell.modifier)})).collect();
            let frame = json!({"label":"Terminal-Test-Render","width":width,"height":height,"state":state,"cells":cells});
            std::fs::write(
                format!("{directory}/{state}-{width}x{height}.json"),
                serde_json::to_vec_pretty(&frame)?,
            )?;
        }
    }
    Ok(())
}
