//! A scripted product walkthrough using Amber's real model and Ratatui renderer.
//! No shell process, file context, provider request or command execution occurs.
use amber_shell::{
    model::{App, Reply},
    ui,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use serde::Serialize;
use std::path::Path;

const COLUMNS: u16 = 56;
const UI_ROWS: u16 = 16;
const PROMPT: &str = " ~/website % ";

#[derive(Serialize, PartialEq)]
struct CellStyle {
    fg: String,
    bg: String,
    bold: bool,
    reversed: bool,
    underlined: bool,
}
#[derive(Serialize)]
struct Shot {
    duration_ms: u32,
    step: usize,
    title: &'static str,
    hint: &'static str,
    key: &'static str,
    cells: Vec<(String, usize)>,
    cursor: Option<(u16, u16)>,
    kind: &'static str,
    command: Option<String>,
}
#[derive(Serialize)]
struct Demo {
    columns: u16,
    rows: u16,
    simulated: bool,
    executed: bool,
    styles: Vec<CellStyle>,
    frames: Vec<Shot>,
}
impl Demo {
    fn new() -> Self {
        Self {
            columns: COLUMNS,
            rows: UI_ROWS + 2,
            simulated: true,
            executed: false,
            styles: vec![],
            frames: vec![],
        }
    }
    fn cells(&mut self, terminal: &Terminal<TestBackend>) -> Vec<(String, usize)> {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| {
                let style = CellStyle {
                    fg: format!("{:?}", cell.fg),
                    bg: format!("{:?}", cell.bg),
                    bold: cell.modifier.contains(Modifier::BOLD),
                    reversed: cell.modifier.contains(Modifier::REVERSED),
                    underlined: cell.modifier.contains(Modifier::UNDERLINED),
                };
                let id = if let Some(id) = self.styles.iter().position(|s| *s == style) {
                    id
                } else {
                    self.styles.push(style);
                    self.styles.len() - 1
                };
                (cell.symbol().to_owned(), id)
            })
            .collect()
    }
    fn shell(
        &mut self,
        command: &str,
        duration_ms: u32,
        key: &'static str,
        cursor: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(TestBackend::new(COLUMNS, UI_ROWS + 2))?;
        terminal.draw(|f| {
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(PROMPT, Style::default().fg(Color::Indexed(248))),
                    Span::styled(
                        command,
                        Style::default()
                            .fg(Color::Indexed(223))
                            .add_modifier(Modifier::BOLD),
                    ),
                ])),
                f.area(),
            )
        })?;
        let cells = self.cells(&terminal);
        let handoff = !command.is_empty();
        self.frames.push(Shot {
            duration_ms,
            step: if handoff { 3 } else { 0 },
            title: if handoff {
                "Your command. Ready to edit."
            } else {
                "An assistant. Right in your terminal."
            },
            hint: if handoff {
                "Press Enter in the shell to run it."
            } else {
                "Open Amber at the shell prompt."
            },
            key,
            cells,
            cursor: cursor.then_some((PROMPT.len() as u16 + command.len() as u16, 0)),
            kind: "shell",
            command: handoff.then(|| command.into()),
        });
        Ok(())
    }
    fn amber(
        &mut self,
        app: &mut App,
        duration_ms: u32,
        step: usize,
        key: &'static str,
        show_cursor: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut ui_terminal = Terminal::new(TestBackend::new(COLUMNS, UI_ROWS))?;
        ui_terminal.draw(|f| ui::render(f, app))?;
        let cursor = ui_terminal.get_cursor_position()?;
        // Preserve the free shell prompt above Amber's native inline surface.
        let mut prefix = Terminal::new(TestBackend::new(COLUMNS, 2))?;
        prefix.draw(|f| {
            f.render_widget(
                Paragraph::new(PROMPT).style(Style::default().fg(Color::Indexed(248))),
                f.area(),
            )
        })?;
        let mut cells = self.cells(&prefix);
        cells.extend(self.cells(&ui_terminal));
        self.frames.push(Shot {
            duration_ms,
            step,
            title: match step {
                0 => "An assistant. Right in your terminal.",
                1 => "Ask in your own words.",
                2 => "Ask a follow-up. Amber remembers.",
                _ => "Your terminal. Your choice.",
            },
            hint: match step {
                0 => "Open Amber at the shell prompt.",
                1 => "A simple command. Clearly explained.",
                2 => "More detail when you need it.",
                _ => "Use Ctrl-Y to move it to the shell.",
            },
            key,
            cells,
            cursor: show_cursor.then_some((cursor.x, cursor.y + 2)),
            kind: "amber",
            command: None,
        });
        Ok(())
    }
    fn type_question(
        &mut self,
        app: &mut App,
        question: &str,
        step: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let chars: Vec<_> = question.chars().collect();
        for chunk in chars.chunks(2) {
            app.editor.insert(&chunk.iter().collect::<String>());
            self.amber(app, 80, step, "", true)?;
        }
        self.amber(app, 420, step, "Enter", true)
    }
    fn answer(
        &mut self,
        app: &mut App,
        explanation: &str,
        command: &str,
        step: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        app.begin()?;
        for i in 0..4 {
            app.indicator_frame = i;
            self.amber(app, 140, step, "", false)?;
        }
        app.delta("{\"explanation\":\"")?;
        let chars: Vec<_> = explanation.chars().collect();
        for (i, chunk) in chars.chunks(4).enumerate() {
            app.delta(&chunk.iter().collect::<String>())?;
            app.indicator_frame = i % 4;
            self.amber(app, 60, step, "", false)?;
        }
        app.complete(Reply {
            explanation: explanation.into(),
            command: command.into(),
        })?;
        for cursor in [true, false, true, false] {
            self.amber(app, 450, step, "", cursor)?;
        }
        Ok(())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "work/product-demo/timeline.json".into());
    let mut demo = Demo::new();
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.ascii = false;
    app.ascii_input = false;
    app.no_color = false;
    app.reduced_motion = false;
    demo.amber(&mut app, 900, 0, "Ctrl-X  →  Ctrl-A", true)?;
    demo.amber(&mut app, 450, 1, "", true)?;
    demo.type_question(&mut app, "How do I list files and folders?", 1)?;
    demo.answer(&mut app, "ls lists files and folders.", "ls", 1)?;
    demo.type_question(&mut app, "And hidden ones?", 2)?;
    demo.answer(&mut app, "Use -a to include hidden entries.", "ls -a", 2)?;
    demo.amber(&mut app, 800, 3, "Ctrl-Y", false)?;
    let command = app
        .selected_command()
        .ok_or("No complete command to transfer")?
        .to_owned();
    assert_eq!(command, "ls -a");
    demo.shell(&command, 600, "Editable at the prompt", true)?;
    // A small manual edit makes the editable handoff visible; still no Enter.
    demo.shell(&format!("{command}l"), 650, "Edit command", true)?;
    demo.shell(&command, 900, "Not executed yet", true)?;
    demo.shell(&command, 550, "Not executed yet", false)?;
    let path = Path::new(&output);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec(&demo)?)?;
    println!(
        "{}: {} frames, {} ms; synthetic data, no command executed",
        output,
        demo.frames.len(),
        demo.frames.iter().map(|f| f.duration_ms).sum::<u32>()
    );
    Ok(())
}
