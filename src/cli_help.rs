pub const MAIN: &str = r#"Inline chat for zsh and Apple Terminal.

Usage:
  amber-shell [OPTIONS]
  amber-shell <COMMAND> [OPTIONS]

Get started
  amber-shell login                  Log in with ChatGPT in your browser
  amber-shell                        Open Amber and type a question

  With zsh integration installed, press Ctrl-X, then Ctrl-A at a free prompt.

Commands
  login [status | --device-auth]      Sign in or check Amber's login
  logout                             Sign out of Amber
  ask --question TEXT [OPTIONS]      Ask once and print a JSON reply
  session -- /bin/zsh -i              Start a shell with output capture
  doctor                             Check Codex CLI and Amber's login

  Run amber-shell <command> --help for details and examples.

Keys
  Ctrl-X, then Ctrl-A   Open Amber from the normal zsh prompt
  Enter                Ask; in history, accept the selected command
  Ctrl-Y               Accept a complete command into the shell prompt
  Tab                  Switch between the question and history
  Up / Down            Read history and long replies
  Left / Right         Select a command in history
  Ctrl-O               Choose a model or Fast mode
  Ctrl-F               Toggle Fast while the model picker is open
  Ctrl-K               Choose context: A automatic, T terminal, X off
  Ctrl-E               Follow the latest reply
  Esc / Ctrl-C         Cancel and return to the shell

  After Ctrl-Y, edit the command if needed. Enter at the shell prompt runs it.

Context & language
  The interface is English. Replies follow the language of your current
  question. Use Ctrl-K to choose which context accompanies your question.
  Saved history is separate from context for a new conversation.

Advanced integration options
  --simulate               Use simulated replies without a network request
  --history-file PATH      Read NUL-separated shell commands, up to 32 KiB
  --capture-socket PATH    Read bounded output from the current shell session
  --result-file PATH       Write original command bytes plus NUL on acceptance
  --version, -V            Show the version
  --help, -h               Show help
"#;

pub const LOGIN: &str = r#"Log in with ChatGPT for Amber Shell.

Usage:
  amber-shell login
  amber-shell login status
  amber-shell login --device-auth

Browser login
  Run amber-shell login to open the official sign-in flow in your browser.
  Choose Log in with ChatGPT, finish signing in, then return to your terminal.
  A personal Codex login is not reused. Sign in once for Amber Shell.

Device login
  Use --device-auth if browser login is unavailable. Follow the URL and code
  printed by the official Codex CLI.

Login status
  amber-shell login status reports Amber's login without a model request.

Credential scope
  Amber uses the official Codex CLI with a dedicated CODEX_HOME and private
  file store at:
    ${XDG_CONFIG_HOME:-$HOME/.config}/amber-shell/codex

  Your personal Codex credentials are kept separate. API keys and arbitrary
  Codex login options are not accepted by this command.

Codex executable
  AMBER_CODEX_BIN overrides the default: /opt/homebrew/bin/codex

Options
  --help, -h               Show this help
"#;

pub const LOGOUT: &str = r#"Log out of Amber Shell's ChatGPT account.

Usage:
  amber-shell logout

Credential scope
  Removes only Amber's dedicated Codex login from:
    ${XDG_CONFIG_HOME:-$HOME/.config}/amber-shell/codex

  Your personal Codex login and browser ChatGPT session stay signed in.
  Amber's local history and settings are retained.

  To sign in again, run amber-shell login.

Options
  --help, -h               Show this help
"#;

pub const ASK: &str = r#"Ask one question and print a JSON reply.

Usage:
  amber-shell ask --question TEXT [OPTIONS]

Examples
  amber-shell ask --question "How do I list files?"
  amber-shell ask --question "Explain this error" --context-file error.txt
  amber-shell ask --question "Show the current directory" --simulate

Options
  --question TEXT          Required question, up to 8 KiB
  --model ID               Use this model instead of the saved selection
  --fast                   Request the supported Fast tier
  --context-file PATH      Read selected UTF-8 context, up to 48 KiB
  --conversation-file PATH Read JSON question/reply pairs, up to 6 MiB
  --simulate               Use a simulated reply without a network request
  --help, -h               Show this help

Context & language
  The reply follows the language of the current question. JSON field names
  remain English. Only explicitly supplied context and conversation are sent.

  The JSON reply contains explanation and command. No command is executed.
  Run amber-shell login before making live requests.
"#;

pub const SESSION: &str = r#"Start a shell with terminal output capture.

Usage:
  amber-shell session -- /bin/zsh -i

Get started
  amber-shell session -- /bin/zsh -i

  With zsh integration installed, press Ctrl-X, then Ctrl-A at a free prompt
  to open Amber. Ctrl-K chooses context: A automatic, T terminal, X off.

  This session makes bounded terminal output available as context. Review
  the context selection before asking a question.

  Ctrl-Y accepts a suggested command into the editable shell prompt. Edit it
  if needed, then press Enter at the normal shell prompt to run it.

Options
  --                       Separate Amber arguments from the shell command
  --help, -h               Show this help
"#;

pub const DOCTOR: &str = r#"Check the Codex CLI and Amber Shell's login.

Usage:
  amber-shell doctor

Checks
  Prints the selected Codex CLI version and Amber's dedicated login status.
  Does not send a model request or use your personal Codex login.

  If Amber is signed out, run amber-shell login to log in with ChatGPT.

Codex executable
  AMBER_CODEX_BIN overrides the default: /opt/homebrew/bin/codex

Options
  --help, -h               Show this help
"#;
