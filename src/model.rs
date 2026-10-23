use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    pub explanation: String,
    pub command: String,
}

pub fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("Empty command".into());
    }
    if command.len() > 16 * 1024 {
        return Err("Command exceeds 16 KiB".into());
    }
    if command
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("Command contains unsupported control characters".into());
    }
    Ok(())
}

pub fn safe_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

#[derive(Default, Debug)]
pub struct Editor {
    pub text: String,
    pub cursor: usize,
}
impl Editor {
    pub fn insert(&mut self, text: &str) -> bool {
        let text = safe_text(text);
        if self.text.len() + text.len() > 8192 {
            return false;
        }
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
        true
    }
    pub fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
    }
    pub fn right(&mut self) {
        self.cursor += self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(0, str::len);
    }
    pub fn home(&mut self) {
        self.cursor = 0;
    }
    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
    }
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub question: String,
    pub stream: String,
    pub reply: Option<Reply>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerStatus {
    Ready,
    Waiting,
    Streaming,
    Complete,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInState {
    Required,
    Starting,
    Waiting,
    Failed(String),
}

struct ContextSource {
    cwd: std::path::PathBuf,
    snapshot: crate::context::Context,
    project: Option<crate::projects::ProjectIndex>,
}

pub struct App {
    pub editor: Editor,
    pub turns: Vec<Turn>,
    pub past_turns: Vec<Turn>,
    pub shell_commands: Vec<String>,
    pub history_cwd: Option<std::path::PathBuf>,
    pub active: bool,
    pub sign_in: Option<SignInState>,
    pub selected: Option<usize>,
    pub manual_selection: bool,
    pub command_focus: bool,
    pub reviewing: bool,
    context_source: Option<ContextSource>,
    pub context_choice: usize,
    pub contexts: [String; 4],
    pub context_details: [String; 4],
    pub context_counts: [(usize, usize); 4],
    pub context_folder: String,
    pub context_raw: bool,
    pub context_error: String,
    pub simulated: bool,
    pub status: String,
    pub follow: bool,
    pub anchor: usize,
    pub scroll_delta: i32,
    pub last_top: usize,
    pub ascii: bool,
    pub ascii_input: bool,
    pub no_color: bool,
    pub cursor_row: u16,
    pub model_id: String,
    pub fast: bool,
    pub picker_fast: bool,
    pub models: Vec<crate::provider::ModelInfo>,
    pub model_picker: bool,
    pub model_cursor: usize,
    pub model_loading: bool,
    pub model_error: String,
    pub selection_reveal: bool,
    pub indicator_frame: usize,
    pub reduced_motion: bool,
}
impl App {
    pub fn new(contexts: [String; 4], simulated: bool) -> Self {
        Self {
            editor: Editor::default(),
            turns: vec![],
            past_turns: vec![],
            shell_commands: vec![],
            history_cwd: None,
            active: false,
            sign_in: None,
            selected: None,
            manual_selection: false,
            command_focus: false,
            reviewing: false,
            context_source: None,
            context_choice: 3,
            context_details: contexts.clone(),
            context_counts: [(0, 0); 4],
            context_folder: String::new(),
            context_raw: false,
            context_error: String::new(),
            contexts,
            simulated,
            status: String::new(),
            follow: true,
            anchor: 0,
            scroll_delta: 0,
            last_top: 0,
            cursor_row: 0,
            model_id: crate::provider::DEFAULT_MODEL.into(),
            fast: false,
            picker_fast: false,
            models: vec![],
            model_picker: false,
            model_cursor: 0,
            model_loading: false,
            model_error: String::new(),
            selection_reveal: false,
            indicator_frame: 0,
            reduced_motion: std::env::var_os("AMBER_SHELL_REDUCED_MOTION").is_some(),
            ascii: std::env::var("AMBER_SHELL_ASCII").as_deref() == Ok("1"),
            ascii_input: std::env::var("AMBER_SHELL_ASCII").as_deref() == Ok("1"),
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }
    pub fn from_shell_context(cwd: &std::path::Path, history: &[String], simulated: bool) -> Self {
        Self::from_shell_context_with_outputs(cwd, history, &[], simulated)
    }
    pub fn from_shell_context_with_outputs(
        cwd: &std::path::Path,
        history: &[String],
        outputs: &[crate::capture::CaptureRecord],
        simulated: bool,
    ) -> Self {
        let mut snapshot = crate::context::collect_shell(cwd, history);
        snapshot.outputs = outputs
            .iter()
            .filter(|r| crate::capture::safe_record(r))
            .rev()
            .cloned()
            .collect();
        snapshot.outputs.reverse();
        let mut app = Self::new(std::array::from_fn(|_| String::new()), simulated);
        app.context_folder = cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| snapshot.cwd.clone());
        app.shell_commands = snapshot
            .history
            .iter()
            .filter(|c| validate_command(c).is_ok())
            .cloned()
            .collect();
        app.context_source = Some(ContextSource {
            cwd: cwd.to_owned(),
            snapshot,
            project: None,
        });
        app.set_context_choice(3);
        app
    }
    pub fn context_mode(&self) -> crate::settings::ContextMode {
        use crate::settings::ContextMode;
        match self.context_choice {
            2 => ContextMode::Terminal,
            4 => ContextMode::Off,
            _ => ContextMode::Automatic,
        }
    }
    pub fn set_context_choice(&mut self, choice: usize) {
        self.context_choice = match choice {
            2 | 4 => choice,
            _ => 3,
        };
        self.anchor = 0;
        let question = self.editor.text.clone();
        self.prepare_context(&question);
    }
    fn prepare_context(&mut self, question: &str) {
        if self.context_choice == 4 {
            return;
        }
        let Some(source) = &mut self.context_source else {
            return;
        };
        let automatic = self.context_choice == 3;
        source.snapshot.files.clear();
        let mut limited = false;
        if automatic && crate::projects::relevant(question) {
            let project = source
                .project
                .get_or_insert_with(|| crate::projects::ProjectIndex::discover(&source.cwd));
            source.snapshot.files = project.select(question);
            limited = project.limited;
        }
        let payload = source.snapshot.payload(automatic, true);
        self.context_counts[self.context_choice] = (payload.files.len(), payload.commands.len());
        self.context_details[self.context_choice] = format!(
            "{}{}",
            payload.details,
            if limited {
                "\n\nProject search limited; some subfolders may be missing. Specify the exact folder or change to it if needed."
            } else {
                ""
            }
        );
        self.contexts[self.context_choice] = payload.text;
        if limited {
            self.contexts[self.context_choice].push_str("\nProject search limited (depth/count/time). Do not invent missing scripts; ask for the folder if the project is unclear.\n");
        }
    }
    pub fn selected_context(&self) -> &str {
        self.contexts
            .get(self.context_choice)
            .map_or("", String::as_str)
    }
    pub fn context_preview(&self) -> String {
        if self.context_raw && self.context_choice != 4 && self.context_error.is_empty() {
            return self.selected_context().into();
        }
        let mut text = String::new();
        for (key, mode) in [
            ('A', crate::settings::ContextMode::Automatic),
            ('T', crate::settings::ContextMode::Terminal),
            ('X', crate::settings::ContextMode::Off),
        ] {
            text.push_str(&format!(
                "{} {}  {}\n",
                match (self.ascii, mode == self.context_mode()) {
                    (true, true) => "[x]",
                    (true, false) => "[ ]",
                    (false, true) => "●",
                    (false, false) => "○",
                },
                key,
                mode.label()
            ));
        }
        text.push('\n');
        if !self.context_error.is_empty() {
            text.push_str(&format!("Setting not saved:\n{}\n\n", self.context_error));
        }
        text.push_str(match self.context_mode() {
            crate::settings::ContextMode::Automatic => "Terminal + relevant project info.\nSubfolders only for project questions.\n\n",
            crate::settings::ContextMode::Terminal => "Folder, commands and output.\nProject files are not read.\n\n",
            crate::settings::ContextMode::Off => "No new context from the next question.\nEarlier questions and answers stay in the conversation.\nClose and reopen Amber for a fresh conversation.\n",
        });
        if self.context_choice != 4 {
            text.push_str(&self.context_details[self.context_choice]);
        }
        text
    }
    pub fn context_summary(&self, _width: u16) -> String {
        format!("{} · Ctrl-K Context", self.context_mode().label())
    }
    pub fn answer_status(&self) -> AnswerStatus {
        if let Some(state) = &self.sign_in {
            return match state {
                SignInState::Required => AnswerStatus::Ready,
                SignInState::Starting | SignInState::Waiting => AnswerStatus::Waiting,
                SignInState::Failed(_) => AnswerStatus::Error,
            };
        }
        if self.active {
            if self.turns.last().is_some_and(|t| !t.stream.is_empty()) {
                AnswerStatus::Streaming
            } else {
                AnswerStatus::Waiting
            }
        } else if self.turns.last().is_some_and(|t| t.error.is_some()) {
            AnswerStatus::Error
        } else if self.turns.last().is_some_and(|t| t.reply.is_some()) {
            AnswerStatus::Complete
        } else {
            AnswerStatus::Ready
        }
    }
    pub fn advance_indicator(&mut self) -> bool {
        let signing_in = matches!(
            self.sign_in,
            Some(SignInState::Starting | SignInState::Waiting)
        );
        if (!self.active && !signing_in) || self.reduced_motion {
            return false;
        }
        self.indicator_frame = (self.indicator_frame + 1) % 4;
        true
    }
    pub fn model_label(&self) -> String {
        if let Some(model) = self.models.iter().find(|m| m.id == self.model_id) {
            return model.label.clone();
        }
        if let Some(rest) = self.model_id.strip_prefix("gpt-") {
            let mut words = rest.split('-');
            let version = words.next().unwrap_or(rest);
            let suffix = words
                .map(|word| {
                    let mut chars = word.chars();
                    chars
                        .next()
                        .map(|c| c.to_uppercase().to_string() + chars.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            return format!("GPT-{version} {suffix}").trim_end().to_string();
        }
        self.model_id.clone()
    }
    pub fn move_model(&mut self, direction: i32) {
        self.model_cursor = (self.model_cursor as i32 + direction)
            .clamp(0, self.models.len().saturating_sub(1) as i32)
            as usize;
    }
    pub fn begin(&mut self) -> Result<(String, Vec<(String, String)>), String> {
        if self.sign_in.is_some() {
            return Err("Log in with ChatGPT to continue".into());
        }
        if self.active {
            return Err("Answer still in progress".into());
        }
        if self.turns.len() >= 20 {
            return Err("20 questions reached – please open a new conversation".into());
        }
        if self.editor.text.trim().is_empty() {
            return Err("Please enter a question".into());
        }
        let conversation = self
            .turns
            .iter()
            .filter_map(|t| {
                t.reply.as_ref().map(|r| {
                    (
                        t.question.clone(),
                        serde_json::to_string(r).unwrap_or_default(),
                    )
                })
            })
            .collect();
        let question = self.editor.take();
        self.prepare_context(&question);
        self.turns.push(Turn {
            question: question.clone(),
            stream: String::new(),
            reply: None,
            error: None,
        });
        self.active = true;
        self.indicator_frame = 0;
        self.status = "Receiving answer …".into();
        Ok((question, conversation))
    }
    pub fn delta(&mut self, text: &str) -> Result<(), String> {
        let turn = self.turns.last_mut().ok_or("No active question")?;
        if turn.stream.len() + text.len() > 256 * 1024 {
            return Err("Answer exceeds 256 KiB".into());
        }
        turn.stream.push_str(text);
        Ok(())
    }
    pub fn complete(&mut self, reply: Reply) -> Result<(), String> {
        validate_command(&reply.command)?;
        if reply.explanation.len() + reply.command.len() > 256 * 1024 {
            return Err("Answer too large".into());
        }
        let index = self
            .turns
            .len()
            .checked_sub(1)
            .ok_or("No active question")?;
        self.turns[index].reply = Some(reply);
        self.active = false;
        self.status = "Ready · Ask a follow-up or use the command".into();
        if !self.manual_selection || self.selected.is_none() {
            self.selected = Some(self.history_base() + index);
        }
        Ok(())
    }
    pub fn fail(&mut self, message: String) {
        self.active = false;
        self.status = safe_text(&message);
        if let Some(turn) = self.turns.last_mut() {
            turn.error = Some(self.status.clone());
        }
    }
    pub fn history_base(&self) -> usize {
        self.shell_commands.len() + self.past_turns.len()
    }
    fn command_choices(&self) -> Vec<usize> {
        (0..self.shell_commands.len())
            .chain(
                self.past_turns
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.reply.is_some())
                    .map(|(i, _)| self.shell_commands.len() + i),
            )
            .chain(
                self.turns
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.reply.is_some())
                    .map(|(i, _)| self.history_base() + i),
            )
            .collect()
    }
    pub fn history_empty(&self) -> bool {
        self.command_choices().is_empty()
    }
    pub fn leave_history(&mut self) {
        if !self.command_focus {
            return;
        }
        self.command_focus = false;
        // Ctrl-Y must target a command in the currently visible conversation.
        self.selected = self
            .turns
            .iter()
            .rposition(|t| t.reply.is_some())
            .map(|i| self.history_base() + i);
        self.manual_selection = false;
        self.selection_reveal = false;
        self.follow = true;
        self.anchor = usize::MAX;
    }
    pub fn toggle_history(&mut self) {
        if self.command_focus {
            self.leave_history();
        } else {
            self.command_focus = true;
            if self.selected.is_none() {
                self.selected = self.command_choices().last().copied();
            }
            self.selection_reveal = true;
        }
    }
    pub fn selected_command(&self) -> Option<&str> {
        let i = self.selected?;
        if i < self.shell_commands.len() {
            return self.shell_commands.get(i).map(String::as_str);
        }
        let turn = if i < self.history_base() {
            self.past_turns.get(i - self.shell_commands.len())
        } else {
            self.turns.get(i - self.history_base())
        };
        turn.and_then(|t| t.reply.as_ref())
            .map(|r| r.command.as_str())
    }
    pub fn select(&mut self, direction: i32) {
        let choices = self.command_choices();
        if choices.is_empty() {
            return;
        }
        let at = choices
            .iter()
            .position(|i| Some(*i) == self.selected)
            .unwrap_or(0);
        let next = (at as i32 + direction).clamp(0, choices.len() as i32 - 1) as usize;
        self.selected = Some(choices[next]);
        self.manual_selection = true;
        self.selection_reveal = true;
    }
}
