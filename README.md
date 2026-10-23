# amber-shell

A compact terminal assistant for **macOS, Apple Terminal, and Zsh**: ask a question, read the explanation, and insert the suggested command into your editable shell prompt. Amber uses the official Codex CLI with its own **Log in with ChatGPT** flow.

Amber's interface stays in English. Replies follow the language of your current question; you can explicitly request a different response language.

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Ratatui](https://img.shields.io/badge/Ratatui-F8C76D?style=for-the-badge&logoColor=171717)](https://ratatui.rs/)
[![macOS ARM64](https://img.shields.io/badge/macOS-ARM64-20232A?style=for-the-badge&logo=apple&logoColor=white)](#installation)
[![Homebrew](https://img.shields.io/badge/Homebrew-FBB040?style=for-the-badge&logo=homebrew&logoColor=171717)](https://github.com/marcdon2002/homebrew-tap)
[![Zsh](https://img.shields.io/badge/Zsh-F15A24?style=for-the-badge&logo=zsh&logoColor=white)](https://www.zsh.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-F8C76D?style=for-the-badge)](./LICENSE)
[![Codex](https://img.shields.io/badge/Codex-ChatGPT_Login-10A37F?style=for-the-badge)](#chatgpt-login)

[Installation](#installation) · [ChatGPT login](#chatgpt-login) · [Usage](#usage) · [Model selection](#model-selection) · [Repository structure](#repository-structure) · [Development](#development) · [Help](#help)

> [!TIP]
> **Ctrl-X, then Ctrl-A** opens Amber directly at the prompt. **Ctrl-Y** inserts the selected command. It only runs when you separately press **Enter in the shell**. Escape or Ctrl-C preserves your previous input and cursor position.

![Open Amber, ask a question, follow up, and insert an editable command into the shell](./assets/amber-demo.gif)

*Product flow from the actual Ratatui interface, with simulated responses and illustrative wait times. Ctrl-Y inserts the command as editable text; it only runs when you press Enter in the shell.*

## Overview

Amber stays in a small inline area in the terminal. Each exchange forms a single block marked by a short vertical line. “You” and “Amber” identify the roles; questions and answers share the same text alignment, with commands slightly indented below. The compact Amber logo has its own wordmark and an independently animated status symbol. The input keeps its rounded border. The area adapts to the content height; use the arrow keys to read longer answers and commands. The model and Fast mode remain visible on the right.

The Tab history combines previous Amber questions and their suggested commands from the same directory with recent filtered shell commands. You can ask follow-up questions in the current conversation. Opening a new dialog starts a fresh model conversation. Recent commands and limited terminal output are available as context so Amber can help explain errors.

Amber suggests the simplest suitable standard command first: “How do I list all directories?” returns `ls`. Ask specifically for additional options or more complex alternatives.

A small Rust helper runs for each terminal to capture output, waiting for events while idle. It keeps snippets entirely in memory; no additional capture process starts for each command. Codex starts for a request and exits afterward.

## Installation

**Requirements:** macOS 13 or later on Apple Silicon, Zsh, the official Codex CLI, and a ChatGPT account with Codex access. You can keep using your existing Oh My Zsh configuration.

### With Homebrew

```sh
brew install marcdon2002/tap/amber-shell
amber-shell-setup
```

If the official Codex CLI is not installed yet:

```sh
brew install --cask codex
```

Open a **new terminal window** and press **Ctrl-X, then Ctrl-A**. On first open, Amber shows **Log in with ChatGPT**. Press **Enter** to sign in through the official browser flow; Amber waits and opens the question input in the same dialog when login completes.

Amber can reuse an existing Codex installation. Its login is separate from your personal Codex login, so sign into Amber once even if you already use Codex. No separate login command is required.

Homebrew downloads the prebuilt ARM64 package and verifies its SHA256 checksum; installation does not require a Rust toolchain. Packages are distributed through the project's [Homebrew tap](https://github.com/marcdon2002/homebrew-tap). Homebrew packages for Intel Macs and Linux are not currently available.

`amber-shell-setup` backs up existing `.zshenv` and `.zshrc` files, adds the marked Amber blocks, and can migrate a previous installation under `~/.local`. It only replaces unchanged blocks created by the installer; custom changes and symlinks require you to specify the appropriate target. The integration uses Homebrew's stable `opt` path, so updates do not require setup again. Setup does not send a model request.

With a previous source installation, `~/.local/bin/amber-shell` may appear before Homebrew in your `PATH`. The configured hotkey explicitly uses the Homebrew binary. For direct CLI calls, remove the old file from your `PATH` after a successful migration, or run `"$(brew --prefix amber-shell)/bin/amber-shell"`.

To update:

```sh
brew update
brew upgrade marcdon2002/tap/amber-shell
```

Then open a new terminal window. To uninstall, first remove the two marked Amber blocks from `.zshenv` and `.zshrc`, then run `brew uninstall amber-shell`. Local settings and history are retained.

### From source

With a Rust toolchain that meets the requirements in [Cargo.toml](./Cargo.toml), run these commands in the repository directory:

```sh
cargo build --release --locked
sh scripts/install.sh
```

The source installer places the binary at `~/.local/bin/amber-shell` and the shell files under `~/.local/share/amber-shell/`. Install the official Codex CLI if needed, then open a new terminal window and press **Ctrl-X, then Ctrl-A**. Amber offers **Log in with ChatGPT** inside the dialog. An early block in `.zshenv` starts output capture; `.zshrc` loads the ZLE widget after the prompt configuration. Personal Zsh startup files still load once; noninteractive shells and scripts run without the capture helper.

<details>
<summary>Symlinked shell files or a different Codex path</summary>

For a symlinked `.zshrc` or `.zshenv`, specify its resolved target; the symlink is preserved:

```sh
amber-shell-setup --zshrc /absolute/path/to/zshrc
# If .zshenv is also symlinked, add --zshenv /absolute/path/to/zshenv.
# Source installation: pass the same options to sh scripts/install.sh.
```

The default Codex path is `/opt/homebrew/bin/codex`. For another official installation, set this before opening Amber:

```sh
export AMBER_CODEX_BIN="/absolute/path/to/codex"
```

Set `AMBER_SHELL_KEY` before loading the plugin to customize the hotkey. Existing key bindings are not overwritten.

</details>

<details>
<summary>Try Amber without a permanent shell integration</summary>

```sh
export AMBER_SHELL_BIN="$PWD/target/release/amber-shell"
source shell/amber-shell.zsh
```

This integration only applies to the current shell and cannot capture earlier output. To try the capture helper without installing it, start a separate shell and load the plugin there:

```sh
"$AMBER_SHELL_BIN" session -- /bin/zsh -i
# In this shell, still in the repository directory:
source shell/amber-shell.zsh
```

For an offline demo:

```sh
export AMBER_SHELL_SIMULATE=1
```

Then use the normal hotkey. Simulation skips login, makes no network requests, is marked **SIM**, and uses separate local history. `unset AMBER_SHELL_SIMULATE` disables demo mode for subsequent launches.

</details>

## ChatGPT login

When Amber has no login, opening it shows **Log in with ChatGPT**. Press **Enter** to launch the official browser login. Amber waits for you to finish and then opens the question input in the same dialog. If you are already signed in, Amber opens the question input directly. Complete this login once after installation or after upgrading from a version that reused your personal Codex login.

Opening Amber checks only local credential-file metadata; it does not contact a server, load the model catalog, or send a model request. Pressing Enter on the login prompt starts a dedicated official Codex login process, which closes after completion or cancellation. Press Enter to retry a failed login or restart one that is still waiting. **Escape** or **Ctrl-C** cancels and returns to the shell with your original input and cursor position preserved.

The official Codex CLI handles authentication and stores the result in Amber's own profile. These optional CLI commands are also available:

```sh
amber-shell login                # Optional browser login from the command line
amber-shell login status         # Show Amber's official login status
amber-shell login --device-auth  # Use the official device-code flow
```

Amber runs the official CLI with a dedicated profile at `${XDG_CONFIG_HOME:-$HOME/.config}/amber-shell/codex/`. The profile directory is private (`0700`), and credential files are stored privately (`0600`) using the CLI's file credential store. Amber ignores an inherited `CODEX_HOME` and API key credentials from the environment. Your personal Codex profile, configuration, and login remain separate. There is no API key fallback.

To switch the account used by Amber:

```sh
amber-shell logout
```

Reopen Amber and press Enter on **Log in with ChatGPT**, or use `amber-shell login`. `amber-shell logout` removes only Amber's login. It keeps Amber's settings and response history, preserves your personal Codex login, and does not sign you out of the ChatGPT website. The browser may still remember the previous account; choose the account you want during the next login.

## Usage

| Key | Action |
| --- | --- |
| Ctrl-X, then Ctrl-A | Open Amber at the shell prompt |
| Enter | Start or retry login; otherwise send a question or follow-up |
| Ctrl-Y | Insert the selected complete command into the shell |
| Tab | Switch between the question and history |
| ↑ / ↓ in history | Read and scroll |
| ← / → in history | Select an Amber suggestion or shell command |
| Enter in history | Insert the selected command into the shell |
| Ctrl-O | Open model selection |
| Ctrl-F in the model picker | Toggle Fast mode when supported |
| Ctrl-K | Configure optional additional context |
| Ctrl-E | Resume following the latest output |
| Escape / Ctrl-C | Cancel and preserve the original shell input |

**Inserting and running a command are separate actions.** Multiline commands are also inserted as editable text. The visible marker is not part of the command. You can prepare a follow-up while an answer is being generated.

### Context and history

**Automatic** is the default: Amber receives the current directory, recent eligible shell commands, and captured output with exit status. There is no fixed limit of three outputs. Instead, a size budget determines how many steps are retained; long outputs are trimmed to their final section and marked. Commands already included with an output are not appended again as shell history.

For project questions, Amber adds relevant information from the current directory, its subdirectories, and parent manifests within the Git project. For “How do I start the npm web project?”, this might include `apps/web/package.json` with the actual scripts and the workspace's package manager. `package.json` is condensed to relevant fields; source code, lockfile contents, and complete dependency trees are not read. Simple questions such as “How do I list directories?” do not trigger a project search.

**Ctrl-K** provides one locally saved choice:

| Key | Mode | Effect |
| --- | --- | --- |
| A | Automatic | Terminal context and relevant project information for project questions |
| T | Terminal only | Directory, commands, and captured output; no project files |
| X | Off | Send no new additional context |

**Tab** shows the actual attachment for the current input; Enter or Escape returns to the question. Your choice applies from the next question and persists when you reopen Amber. Information already sent remains in the current conversation; close and reopen Amber to start over.

Search starts only for a project question, and its results are reused within the dialog. There is no persistent file index, background scanner, or additional model call. Ignored directories, `node_modules`, build directories, and symlinks are skipped. Git ignore rules also apply in directories without a repository. Amber indicates when a search reaches its limits; changing to the relevant subdirectory narrows the search in large projects.

<details>
<summary>Search and size limits</summary>

- Terminal ring buffer: up to **24 KiB serialized**, with no fixed entry count; up to **4 KiB of output tail** per command. The model attachment receives up to **12 KiB** of this, with further output trimming if needed.
- Shell history: up to the last **20 eligible commands**, with an **8 KiB** limit.
- Project search: up to **4 subdirectory levels**, **96 directories**, **1,024 entries**, **24 small manifest or documentation files**, and a **150 ms** time budget checked between file operations.
- Per question: up to **4 project file excerpts** totaling **8 KiB**, including JSON escaping and file markers. The entire new context attachment is limited to **48 KiB**.
- Allowed files: `package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `Makefile`, `justfile`, `README.md`, and `README`. README excerpts are only added for relevant questions. The search is not a complete source code analysis.

</details>

Output capture starts with a new terminal session. It keeps snippets entirely in memory and does not write an output log. Opening Amber requests a snapshot; closing the shell discards the snippets. Interactive input, detected full-screen views, secrets, and sensitive paths are omitted; these filters are heuristic. The combined terminal stream includes standard output and standard error, potentially including background output. Output redirected to files is not captured.

Local Amber history holds at most **20 completed responses and 128 KiB** in total. It is filtered by the current directory. Reading old entries does not automatically attach them to a new model request. Shell history supplies commands; output snippets come exclusively from the current capture session and are not saved in the response history.

<details>
<summary>Display and local files</summary>

- `NO_COLOR=1` disables colors.
- The default view uses short vertical lines and a rounded input border. `AMBER_SHELL_ASCII=1` enables plain ASCII characters for borders and markers; `AMBER_SHELL_ASCII=0` uses the default view.
- `AMBER_SHELL_REDUCED_MOTION=1` disables the activity animation.
- `AMBER_SHELL_CAPTURE=0 /bin/zsh -il` starts a new shell without output capture. Use Ctrl-K and X to disable all new additional context.
- The inline area grows to at most 20 lines; in windows with at least 18 lines, it occupies no more than three quarters of the height.
- Settings and history are stored under `${XDG_CONFIG_HOME:-$HOME/.config}/amber-shell/`. Amber's own files are private and saved atomically.
- `settings.json` stores the model, Fast mode, and context choice; `history.json` stores completed Amber responses. Simulation data is kept separately.
- `codex/` holds the separate official CLI profile used for Amber's ChatGPT login.

</details>

## Model selection

**GPT-6 Luna (`gpt-6-luna`) is the default.** Ctrl-O loads the model catalog for the ChatGPT account signed into Amber. Use the arrow keys to select a model; Enter saves both the model and Fast mode, while Escape discards the change. Changes apply from the next question; model selection is disabled while an answer is being generated.

Fast is an optional service mode available only for supported models. It may use more of your quota; the picker displays this notice. See the [official Codex documentation](https://learn.chatgpt.com/docs/agent-configuration/speed) for the current terms.

Amber keeps its official CLI profile separate from your personal Codex configuration and authentication files. The official CLI handles Amber's [ChatGPT login](#chatgpt-login). Amber does not silently switch to another model.

## Repository structure

```text
src/
├── main.rs                 CLI and dialog startup
├── cli_help.rs             English command help and examples
├── auth.rs                 Separate ChatGPT browser login for Amber
├── model.rs                Input, conversation, and command selection
├── ui.rs                   Compact terminal display
├── terminal.rs             Events, resizing, and restoration
├── provider.rs             Official Codex app server
├── context.rs              Limited, inspectable additional context
├── projects.rs             Targeted search and compact project information
├── capture.rs              Filtered output ring buffer in memory
├── session.rs              Event-driven PTY capture
├── history.rs              Private local response history
└── settings.rs             Model, Fast mode, and context settings
shell/amber-shell.zsh       ZLE widget and safe command insertion
shell/amber-session.zsh     Early startup of output capture
scripts/                   Installation and demo generation
examples/render_frames.rs  Reproducible renderer frames
tests/                     Rust, terminal, and ZLE checks
assets/amber-demo.gif       Embedded terminal animation
LICENSE                    MIT license
```

The published repository includes source code, tests, the lockfile, README, and demo animation. Internal documents and verification reports under `docs/`, working files, build output, local history, and credentials remain excluded through [`.gitignore`](./.gitignore).

## Development

For a code change:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
python3 tests/install_e2e.py
XDG_CONFIG_HOME="$(mktemp -d)" cargo test --locked -- --test-threads=1
```

Terminal checks use their own PTYs, synthetic input, and temporary profiles:

```sh
python3 -m pip install --target work/testdeps -r tests/requirements.txt
cargo build --locked
python3 tests/pty_e2e.py
python3 tests/terminal_e2e.py
python3 tests/zle_binary_e2e.py
python3 tests/session_e2e.py
python3 tests/session_bootstrap_e2e.py
```

Tests that send real model requests are explicitly opt-in. They are skipped in normal test runs.

The README animation contains only demo data from the actual Ratatui renderer:

```sh
TERM=xterm-256color cargo run --locked --example render_demo -- work/product-demo/timeline.json
python3 scripts/render-demo.py --input work/product-demo/timeline.json
```

Intermediate files stay under `work/`; only the finished animation at `assets/amber-demo.gif` is versioned.

### Maintaining Homebrew releases

The [release workflow](./.github/workflows/release.yml) automatically tests and builds Amber on a standard ARM64 macOS runner when a version tag such as `v0.9.0` is pushed. The tag must match the version in `Cargo.toml`. The workflow uses only synthetic tests without model requests.

After updating the version in `Cargo.toml` and `Cargo.lock`, running local tests, and committing:

```sh
git tag -a vX.Y.Z -m "Amber Shell X.Y.Z"
git push origin main
git push origin refs/tags/vX.Y.Z
```

A successful run publishes the installation archive, `SHA256SUMS`, and the `amber-shell.rb` generated from that exact build as a GitHub release. It only needs the Amber repository's GitHub token. Existing release files are not overwritten. A manual run under **Actions → Release → Run workflow** checks and builds the selected revision without publishing a release.

Then update `Formula/amber-shell.rb` in the [Homebrew tap](https://github.com/marcdon2002/homebrew-tap) with **the formula generated by this release**. This separate step is deliberately left to the maintainer; no additional tap credentials are stored in the Amber repository. The tap automatically checks changes with a real Homebrew installation, `brew test`, and `brew audit --strict`. The formula test sets up the shell only in a temporary test directory.

The [release packager](./scripts/package-release.py) remains available for local package checks:

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release --locked
python3 scripts/package-release.py
(cd work/release && shasum -a 256 -c SHA256SUMS)
```

The archive contains only the binary, shell integration, installer, and MIT license. The binary must match the version in `Cargo.toml`. Internal documents and working files remain excluded.

## Help

Use the built-in help for commands and options:

```sh
amber-shell --help
amber-shell login --help
amber-shell logout --help
amber-shell doctor --help
amber-shell ask --help
amber-shell session --help
```

Check Amber's login status and the installed Codex version:

```sh
amber-shell login status
amber-shell doctor
# Source installation without a PATH entry: use ~/.local/bin/amber-shell.
```

`doctor` reports the CLI version and Amber's official login status without sending a model request. If Amber needs a login, open Amber and press Enter on **Log in with ChatGPT**. The optional `amber-shell login` command starts browser login from the command line; use `amber-shell login --device-auth` if you prefer the official device-code flow. To change accounts, run `amber-shell logout` and sign in again when you reopen Amber.

After an update, open a new terminal window so the capture helper is updated too. Your shell input is preserved if there are network, account quota, or model permission problems. If output capture cannot be set up at startup, the normal shell still starts.

Report bugs and suggestions in the [issue tracker](https://github.com/marcdon2002/amber-shell/issues).

## License

Amber Shell is available under the [MIT license](./LICENSE).
