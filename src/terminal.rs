use crate::{
    model::App,
    provider::{self, ProviderEvent, ProviderHandle, ProviderRequest},
    ui,
};
use crossterm::{
    cursor::{self, MoveTo, RestorePosition, SavePosition, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    terminal::{self, Clear, ClearType},
};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupted(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

struct Guard {
    area: Rect,
    previous: libc::termios,
    signals: Vec<(i32, libc::sigaction)>,
}
impl Guard {
    fn enter() -> Result<Self, String> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err("Interactive terminal required".into());
        }
        let mut previous = std::mem::MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(0, previous.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        let mut guard = Self {
            area: Rect::default(),
            previous: unsafe { previous.assume_init() },
            signals: vec![],
        };
        INTERRUPTED.store(false, Ordering::SeqCst);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = interrupted as *const () as usize;
            let mut old = std::mem::MaybeUninit::uninit();
            if unsafe { libc::sigaction(signal, &action, old.as_mut_ptr()) } == 0 {
                guard.signals.push((signal, unsafe { old.assume_init() }));
            }
        }
        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        execute!(io::stdout(), EnableBracketedPaste).map_err(|e| e.to_string())?;
        Ok(guard)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        // The terminal maintains this origin while old rows reflow. The shell
        // prompt precedes it; everything below was reserved by this dialog.
        if self.area.height > 0 {
            let _ = execute!(
                io::stdout(),
                RestorePosition,
                Clear(ClearType::FromCursorDown)
            );
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste, Show);
        let _ = terminal::disable_raw_mode();
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.previous);
        }
        for (signal, old) in &self.signals {
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}
fn clear_owned(area: Rect) -> io::Result<()> {
    let (_, rows) = terminal::size()?;
    let mut out = io::stdout();
    for y in area.y..area.bottom().min(rows) {
        queue!(out, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
    }
    out.flush()
}
fn reserve(top: u16, width: u16, rows: u16) -> io::Result<Rect> {
    let height = if rows < 8 {
        1
    } else {
        let budget = if rows >= 18 {
            rows.saturating_mul(3) / 4
        } else {
            rows.saturating_sub(2)
        };
        20.min(budget)
    };
    let height = height.max(1).min(rows.max(1));
    let top = top.min(rows.saturating_sub(1));
    let mut out = io::stdout();
    execute!(out, MoveTo(0, top))?;
    // Normal line feeds reserve space, preserving existing terminal scrollback.
    for _ in 1..height {
        write!(out, "\r\n")?;
    }
    let area = Rect::new(0, top.min(rows.saturating_sub(height)), width, height);
    execute!(out, MoveTo(0, area.y), SavePosition)?;
    out.flush()?;
    Ok(area)
}
fn fixed(area: Rect) -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )
}

struct CatalogJob {
    events: mpsc::Receiver<Result<Vec<provider::ModelInfo>, String>>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl CatalogJob {
    fn start(simulated: bool) -> Self {
        let (tx, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let worker = thread::spawn(move || {
            let result = if simulated {
                Ok(vec![
                    provider::ModelInfo {
                        id: "gpt-6-luna".into(),
                        label: "GPT-6 Luna".into(),
                        supports_fast: true,
                    },
                    provider::ModelInfo {
                        id: "gpt-6-sol".into(),
                        label: "GPT-6 Sol".into(),
                        supports_fast: false,
                    },
                ])
            } else {
                provider::discover_models_with_cancel(flag)
            };
            let _ = tx.send(result);
        });
        Self {
            events,
            cancel,
            worker: Some(worker),
        }
    }
}
impl Drop for CatalogJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn run(mut app: App, simulated: bool) -> Result<Option<String>, String> {
    let mut guard = Guard::enter()?;
    let (width, rows) = terminal::size().map_err(|e| e.to_string())?;
    let (_, y) = cursor::position().map_err(|e| e.to_string())?;
    guard.area = reserve(y, width, rows).map_err(|e| e.to_string())?;
    app.cursor_row = guard.area.y;
    let mut term = fixed(guard.area).map_err(|e| e.to_string())?;
    let mut provider: Option<ProviderHandle> = None;
    let mut catalog: Option<CatalogJob> = None;
    let mut dirty = true;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut last_pulse = Instant::now();
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if let Some(result) = catalog.as_ref().and_then(|job| job.events.try_recv().ok()) {
            app.model_loading = false;
            match result {
                Ok(models) => {
                    app.model_cursor = models
                        .iter()
                        .position(|m| m.id == app.model_id)
                        .unwrap_or(0);
                    app.models = models;
                    if app.models.is_empty() {
                        app.model_error = "No models available. Ctrl-O reloads.".into();
                    }
                }
                Err(error) => app.model_error = error,
            }
            catalog.take();
            dirty = true;
        }
        let mut finished = false;
        if let Some(handle) = provider.as_ref() {
            // Compare the visible explanation once per batch, rather than reparsing every delta.
            let before_status = app.answer_status();
            let before = app.turns.last().map(|t| ui::streaming_text(&t.stream));
            // Bounded work per iteration keeps input and resize responsive under a fast stream.
            for _ in 0..128 {
                match handle.events.try_recv() {
                    Ok(ProviderEvent::Delta(delta)) => {
                        if let Err(e) = app.delta(&delta) {
                            app.fail(e);
                            dirty = true;
                            finished = true;
                            break;
                        }
                    }
                    Ok(ProviderEvent::Complete(reply)) => {
                        if let Err(e) = app.complete(reply) {
                            app.fail(e);
                        } else if let Some(cwd) = app.history_cwd.as_ref() {
                            let turn = app.turns.last().expect("completed turn");
                            let reply = turn.reply.as_ref().expect("validated reply");
                            let entry = crate::history::HistoryEntry {
                                cwd: String::new(),
                                question: turn.question.clone(),
                                explanation: reply.explanation.clone(),
                                command: reply.command.clone(),
                            };
                            if let Err(error) =
                                crate::history::record_mode(cwd, entry, app.simulated)
                            {
                                app.status = format!("Could not save history: {error}");
                            }
                        }
                        dirty = true;
                        finished = true;
                        break;
                    }
                    Ok(ProviderEvent::Error(e)) => {
                        app.fail(e);
                        dirty = true;
                        finished = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if app.active {
                            app.fail("Provider connection closed".into());
                        }
                        dirty = true;
                        finished = true;
                        break;
                    }
                }
            }
            dirty |= before_status != app.answer_status()
                || before != app.turns.last().map(|t| ui::streaming_text(&t.stream));
        }
        if finished {
            provider.take();
        }
        if last_pulse.elapsed() >= Duration::from_millis(320) {
            dirty |= app.advance_indicator();
            last_pulse = Instant::now();
        }
        if dirty && last_draw.elapsed() >= Duration::from_millis(34) {
            term.draw(|frame| ui::render(frame, &mut app))
                .map_err(|e| e.to_string())?;
            last_draw = Instant::now();
            dirty = false;
        }
        // TTY events wake immediately; only asynchronous model/catalog activity needs frequent polling.
        let poll_interval = if dirty {
            Duration::from_millis(16)
        } else if app.active || catalog.is_some() {
            Duration::from_millis(32)
        } else {
            Duration::from_millis(250)
        };
        if !event::poll(poll_interval).map_err(|e| e.to_string())? {
            continue;
        }
        match event::read().map_err(|e| e.to_string())? {
            Event::Resize(w, h) => {
                // Ratatui 0.30 Inline::resize clears ALL on width shrink. Recreate a fixed
                // inline region instead: only our own rows may be cleared, never the screen.
                // A width change may wrap every old UI row before the input
                // cursor. Subtracting its old row offset therefore loses the
                // origin. Restore our terminal-side anchor instead; the next
                // draw still places the real cursor in the input composer.
                execute!(io::stdout(), RestorePosition).map_err(|e| e.to_string())?;
                let (_, top) = cursor::position().map_err(|e| e.to_string())?;
                // Reflow can expand the old frame beyond its original height.
                // The reserved tail belongs to this dialog, never scrollback.
                let old = Rect::new(0, top, w, h.saturating_sub(top));
                clear_owned(old).map_err(|e| e.to_string())?;
                drop(term);
                guard.area = reserve(top, w, h).map_err(|e| e.to_string())?;
                app.cursor_row = guard.area.y;
                term = fixed(guard.area).map_err(|e| e.to_string())?;
                dirty = true;
            }
            Event::Paste(text) if !app.reviewing && !app.model_picker => {
                app.leave_history();
                if !app.editor.insert(&text) {
                    app.status = "Input exceeds 8 KiB; paste discarded".into();
                }
                dirty = true;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('c') {
                    return Ok(None);
                }
                if key.code == KeyCode::Esc {
                    if app.model_picker {
                        app.model_picker = false;
                        dirty = true;
                        continue;
                    }
                    if app.reviewing {
                        app.reviewing = false;
                        app.follow = true;
                        app.anchor = 0;
                        dirty = true;
                        continue;
                    }
                    return Ok(None);
                }
                if ctrl && key.code == KeyCode::Char('k') {
                    app.reviewing = !app.reviewing;
                    app.model_picker = false;
                    app.context_raw = false;
                    if app.reviewing {
                        app.set_context_choice(app.context_choice);
                    }
                    app.follow = !app.reviewing;
                    app.anchor = 0;
                    dirty = true;
                    continue;
                }
                if ctrl && key.code == KeyCode::Char('o') {
                    if app.active {
                        app.status = "Model change available after the current answer".into();
                    } else {
                        app.model_picker = true;
                        app.picker_fast = app.fast;
                        app.reviewing = false;
                        if catalog.is_none()
                            && (app.models.is_empty() || !app.model_error.is_empty())
                        {
                            app.model_error.clear();
                            app.model_loading = true;
                            catalog = Some(CatalogJob::start(simulated));
                        }
                    }
                    dirty = true;
                    continue;
                }
                if app.model_picker {
                    match key.code {
                        KeyCode::Up => app.move_model(-1),
                        KeyCode::Down => app.move_model(1),
                        KeyCode::Char('f') if ctrl && !app.model_loading => {
                            if app
                                .models
                                .get(app.model_cursor)
                                .is_some_and(|model| model.supports_fast)
                            {
                                app.picker_fast = !app.picker_fast;
                            }
                        }
                        KeyCode::Enter if !app.model_loading => {
                            if let Some(model) = app.models.get(app.model_cursor) {
                                let fast = app.picker_fast && model.supports_fast;
                                match crate::settings::save(&crate::settings::Settings {
                                    model: model.id.clone(),
                                    fast,
                                    context: app.context_mode(),
                                }) {
                                    Ok(()) => {
                                        app.model_id = model.id.clone();
                                        let disabled = app.picker_fast && !fast;
                                        app.fast = fast;
                                        app.model_picker = false;
                                        app.status = if disabled {
                                            "Model saved · Fast unavailable, Standard active".into()
                                        } else {
                                            format!(
                                                "Model saved: {} · {}",
                                                model.label,
                                                if fast { "Fast" } else { "Standard" }
                                            )
                                        };
                                    }
                                    Err(error) => app.model_error = error,
                                }
                            }
                        }
                        _ => {}
                    }
                    dirty = true;
                    continue;
                }
                if app.command_focus
                    && (matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
                        || matches!(key.code, KeyCode::Char(_)) && !ctrl)
                {
                    app.leave_history();
                }
                match key.code {
                    KeyCode::PageUp => app.scroll_delta = app.scroll_delta.saturating_sub(5),
                    KeyCode::PageDown => app.scroll_delta = app.scroll_delta.saturating_add(5),
                    KeyCode::Char('e') if ctrl => {
                        app.follow = true;
                        app.anchor = usize::MAX;
                    }
                    KeyCode::Enter if app.reviewing => {
                        app.reviewing = false;
                        app.follow = true;
                        app.anchor = 0;
                    }
                    KeyCode::Char('a' | 'A' | 't' | 'T' | 'x' | 'X')
                    | KeyCode::Left
                    | KeyCode::Right
                        if app.reviewing =>
                    {
                        let choice = match key.code {
                            KeyCode::Char('a' | 'A') => 3,
                            KeyCode::Char('t' | 'T') => 2,
                            KeyCode::Char('x' | 'X') => 4,
                            KeyCode::Left => match app.context_choice {
                                3 => 4,
                                2 => 3,
                                _ => 2,
                            },
                            _ => match app.context_choice {
                                3 => 2,
                                2 => 4,
                                _ => 3,
                            },
                        };
                        let mut settings = crate::settings::Settings {
                            model: app.model_id.clone(),
                            fast: app.fast,
                            context: match choice {
                                2 => crate::settings::ContextMode::Terminal,
                                4 => crate::settings::ContextMode::Off,
                                _ => crate::settings::ContextMode::Automatic,
                            },
                        };
                        // Retain settings not controlled by this dialog when saving the single choice.
                        match crate::settings::load() {
                            Ok(current) => {
                                settings.model = current.model;
                                settings.fast = current.fast;
                            }
                            Err(error) => {
                                app.context_error = error;
                                dirty = true;
                                continue;
                            }
                        }
                        match crate::settings::save(&settings) {
                            Ok(()) => {
                                app.context_error.clear();
                                app.set_context_choice(choice);
                            }
                            Err(error) => app.context_error = error,
                        }
                    }
                    KeyCode::Up if app.reviewing => {
                        app.scroll_delta = app.scroll_delta.saturating_sub(1)
                    }
                    KeyCode::Down if app.reviewing => {
                        app.scroll_delta = app.scroll_delta.saturating_add(1)
                    }
                    KeyCode::Tab if app.reviewing => {
                        app.context_raw = !app.context_raw;
                        app.anchor = 0;
                    }
                    _ if app.reviewing => {}
                    KeyCode::Char('y') if ctrl => {
                        if let Some(command) = app.selected_command() {
                            return Ok(Some(command.to_owned()));
                        }
                        app.status = "No complete command yet".into();
                    }
                    KeyCode::Enter if app.command_focus => {
                        if let Some(command) = app.selected_command() {
                            return Ok(Some(command.to_owned()));
                        }
                        app.status =
                            "No complete command yet. Tab returns to your question.".into();
                    }
                    KeyCode::Enter => match app.begin() {
                        Ok((question, conversation)) => {
                            provider = Some(provider::start(
                                ProviderRequest {
                                    fast: app.fast,
                                    model: app.model_id.clone(),
                                    question,
                                    conversation,
                                    context: app.selected_context().to_owned(),
                                },
                                simulated,
                            ));
                        }
                        Err(e) => app.status = e,
                    },
                    KeyCode::Tab | KeyCode::BackTab => app.toggle_history(),
                    KeyCode::Up if app.command_focus => {
                        app.scroll_delta = app.scroll_delta.saturating_sub(1)
                    }
                    KeyCode::Down if app.command_focus => {
                        app.scroll_delta = app.scroll_delta.saturating_add(1)
                    }
                    KeyCode::Left if app.command_focus => app.select(-1),
                    KeyCode::Right if app.command_focus => app.select(1),
                    KeyCode::Left => app.editor.left(),
                    KeyCode::Right => app.editor.right(),
                    KeyCode::Home | KeyCode::Char('a') if key.code == KeyCode::Home || ctrl => {
                        app.editor.home()
                    }
                    KeyCode::End => app.editor.end(),
                    KeyCode::Backspace => app.editor.backspace(),
                    KeyCode::Delete => app.editor.delete(),
                    KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                        insert_input(&mut app, &c.to_string());
                    }
                    _ => {}
                }
                dirty = true;
            }
            _ => {}
        }
    }
}

fn insert_input(app: &mut App, text: &str) {
    if !app.editor.insert(text) {
        app.status = "Input limit reached (8 KiB)".into();
    }
}
