use amber_shell::{
    model::{App, Reply},
    ui,
};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::TestBackend, layout::Rect};

fn screen(width: u16, height: u16, app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn layout_is_compact_responsive_and_keeps_original_command() {
    let mut app = App::new(
        [
            "cwd".into(),
            "files".into(),
            "history".into(),
            "both".into(),
        ],
        false,
    );
    app.reviewing = false;
    app.editor.insert("Frage");
    app.begin().unwrap();
    let command = " printf '%s' 'e\u{301} 👩‍💻 漢字'\n";
    app.complete(Reply {
        explanation: "Kurz".into(),
        command: command.into(),
    })
    .unwrap();
    app.editor.insert("Meine Rückfrage");
    assert!(screen(120, 12, &mut app).contains("amber"));
    for (w, h) in [(40, 10), (24, 6), (10, 3), (1, 1), (120, 12)] {
        screen(w, h, &mut app);
    }
    assert_eq!(app.editor.text, "Meine Rückfrage");
    assert_eq!(app.selected_command(), Some(command));
}

#[test]
fn drawing_honors_nonzero_viewport_origin() {
    let mut app = App::new(["CWD".into(), "".into(), "".into(), "CWD".into()], false);
    let mut term = Terminal::with_options(
        TestBackend::new(80, 30),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 15, 80, 12)),
        },
    )
    .unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let b = term.backend().buffer();
    assert_eq!(b[(0, 0)].symbol(), " ");
    assert!((0..80).any(|x| b[(x, 15)].symbol() != " "));
}

#[test]
fn scrolling_reading_anchor_survives_stream_and_rewrap() {
    let text = (0..100)
        .map(|i| format!("Zeile {i} mit genügend Text für einen Umbruch\n"))
        .collect::<String>();
    let mut app = App::new([text.clone(), text.clone(), text.clone(), text], false);
    app.reviewing = true;
    app.follow = false;
    screen(80, 12, &mut app);
    app.scroll_delta = 10;
    screen(80, 12, &mut app);
    let anchor = app.anchor;
    assert!(anchor > 0);
    screen(40, 12, &mut app);
    assert_eq!(app.anchor, anchor);
}

#[test]
fn model_and_actions_remain_discoverable_without_page_keys() {
    let mut app = App::new(["cwd".into(), "".into(), "".into(), "cwd".into()], false);
    app.reviewing = false;
    let wide = screen(100, 9, &mut app);
    assert!(wide.contains("GPT-6 Luna"));
    assert!(wide.contains("Ctrl-O"));
    assert!(wide.contains("Question"));
    assert!(!wide.contains("^Y"));
    app.command_focus = true;
    let history = screen(100, 9, &mut app);
    assert!(history.contains("read"));
    assert!(history.contains("Enter"));
    let narrow = screen(40, 9, &mut app);
    assert!(narrow.contains("Luna"));
    assert!(narrow.contains("Ctrl-O"));
}

#[test]
fn choosing_an_older_command_reveals_it_without_changing_its_bytes() {
    let mut app = App::new(
        [String::new(), String::new(), String::new(), String::new()],
        false,
    );
    app.reviewing = false;
    for name in ["first", "second", "third"] {
        app.editor.insert(name);
        app.begin().unwrap();
        app.complete(Reply {
            explanation: "Eine lange Erklärung zum Lesen und erneuten Umbrechen.".repeat(3),
            command: format!("printf '%s' '{name}'"),
        })
        .unwrap();
    }
    screen(40, 9, &mut app);
    app.select(-2);
    let shown = screen(40, 9, &mut app);
    assert!(shown.contains("> printf '%s' 'first'"));
    assert_eq!(app.selected_command(), Some("printf '%s' 'first'"));
}

#[test]
fn every_wrapped_command_row_is_bold_and_margins_align() {
    use ratatui::style::Modifier;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.reviewing = false;
    app.editor.insert("Test");
    app.begin().unwrap();
    let command = "printf '%s' 'abcdefghijklmnopqrstuvwxyz-long-command'\nprintf '%s' 'tail'";
    app.complete(Reply {
        explanation: "Erklärung".into(),
        command: command.into(),
    })
    .unwrap();
    let mut term = Terminal::new(TestBackend::new(40, 14)).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let buffer = term.backend().buffer();
    let mut code_rows = 0;
    for y in 0..14 {
        let row = (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>();
        assert_eq!(buffer[(0, y)].symbol(), " ");
        assert_eq!(buffer[(39, y)].symbol(), " ");
        if row.contains("printf") || row.contains("command") {
            code_rows += 1;
            for x in 10..39 {
                if buffer[(x, y)].symbol() != " " {
                    assert!(
                        buffer[(x, y)].modifier.contains(Modifier::BOLD),
                        "row not bold: {row}"
                    );
                }
            }
        }
        if row.contains("Erklärung") {
            assert!(!buffer[(1, y)].modifier.contains(Modifier::BOLD));
        }
    }
    assert!(code_rows >= 3);
    assert_eq!(app.selected_command(), Some(command));
}

#[test]
fn ascii_composer_keeps_focus_and_terminal_bold_without_color() {
    use ratatui::style::Modifier;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.reviewing = false;
    app.no_color = true;
    app.ascii = true;
    app.ascii_input = true;
    app.editor.insert("Test");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "Erklärung".into(),
        command: "printf test".into(),
    })
    .unwrap();
    app.selected = None;
    let mut term = Terminal::new(TestBackend::new(40, 14)).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let b = term.backend().buffer();
    let rows: Vec<String> = (0..14)
        .map(|y| (0..40).map(|x| b[(x, y)].symbol()).collect())
        .collect();
    assert!(rows.iter().any(|r| r.contains("Done")));
    assert!(rows.iter().any(|r| r.contains("+ Follow-up ")));
    let y = rows.iter().position(|r| r.contains("printf test")).unwrap() as u16;
    for x in 12..23 {
        assert!(b[(x, y)].modifier.contains(Modifier::BOLD));
    }
    let input_y = rows
        .iter()
        .position(|r| r.contains("Your follow-up"))
        .unwrap() as u16;
    assert_eq!(app.cursor_row, input_y);
    assert!(rows[(input_y - 1) as usize].contains('+'));
    assert!(rows[(input_y + 1) as usize].contains('+'));
}

#[test]
fn grouped_dialogue_preserves_the_original_rounded_input() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.ascii = true;
    app.ascii_input = false;
    app.no_color = true;
    let command = "printf '%s' 'unchanged'\nprintf second";
    app.editor.insert("Wie geht das?");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "Eine kurze Antwort.".into(),
        command: command.into(),
    })
    .unwrap();
    for width in [40, 100] {
        let text = screen(width, 20, &mut app);
        assert!(text.contains("You:   Wie geht das?"), "{text}");
        assert!(text.contains("Amber: Eine kurze Antwort."), "{text}");
        assert!(!text.contains("-- Answer --"), "{text}");
        assert!(!text.contains("-- Command --"), "{text}");
        assert!(text.contains("╭ Follow-up "), "{text}");
        assert!(text.contains("Your follow-up …"), "{text}");
        assert!(text.contains("Enter send · Tab"), "{text}");
        assert_eq!(app.selected_command(), Some(command));
    }
}

#[test]
fn dialogue_gutter_and_logo_align_without_changing_command_bytes() {
    use ratatui::style::{Color, Modifier};
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.ascii = false;
    app.no_color = false;
    app.editor.insert("QUESTION_MARKER");
    app.begin().unwrap();
    let command = "printf first\nprintf second";
    app.complete(Reply {
        explanation: "ANSWER_MARKER".into(),
        command: command.into(),
    })
    .unwrap();
    for width in [40, 100] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        term.draw(|f| ui::render(f, &mut app)).unwrap();
        let b = term.backend().buffer();
        let rows: Vec<String> = (0..20)
            .map(|y| (0..width).map(|x| b[(x, y)].symbol()).collect())
            .collect();
        let q = rows
            .iter()
            .position(|r| r.contains("QUESTION_MARKER"))
            .unwrap();
        let a = rows
            .iter()
            .position(|r| r.contains("ANSWER_MARKER"))
            .unwrap();
        let c = rows
            .iter()
            .position(|r| r.contains("printf first"))
            .unwrap();
        let next = rows
            .iter()
            .position(|r| r.contains("printf second"))
            .unwrap();
        for y in [q, a, c, next] {
            assert_eq!(b[(1, y as u16)].symbol(), "│");
        }
        assert_eq!(b[(10, q as u16)].symbol(), "Q");
        assert_eq!(b[(10, a as u16)].symbol(), "A");
        assert_eq!(b[(10, c as u16)].symbol(), ">");
        assert_eq!(b[(12, c as u16)].symbol(), "p");
        assert_eq!(b[(12, next as u16)].symbol(), "p");
        assert!(b[(5, 0)].modifier.contains(Modifier::BOLD));
        assert_ne!(
            b[(5, 0)].bg,
            Color::Reset,
            "wordmark should stand apart from message roles"
        );
        assert_eq!(app.selected_command(), Some(command));
    }
}

#[test]
fn selection_remains_unambiguous_without_color() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.no_color = true;
    for name in ["first", "second"] {
        app.editor.insert(name);
        app.begin().unwrap();
        app.complete(Reply {
            explanation: "Kurz".into(),
            command: format!("printf {name}"),
        })
        .unwrap();
    }
    app.command_focus = true;
    let shown = screen(100, 20, &mut app);
    assert!(shown.contains("> printf second"));
    assert!(!shown.contains("> printf first"));
    app.select(-1);
    let shown = screen(100, 20, &mut app);
    assert!(shown.contains("> printf first"));
    assert_eq!(app.selected_command(), Some("printf first"));
}

#[test]
fn narrow_view_keeps_command_head_and_can_scroll_to_its_end() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("Question");
    app.begin().unwrap();
    let command = format!("printf {}END", "x".repeat(110));
    app.complete(Reply {
        explanation: "Answer".into(),
        command: command.clone(),
    })
    .unwrap();
    let shown = screen(40, 14, &mut app);
    assert!(shown.contains("> printf"), "{shown}");
    assert!(shown.contains("more"), "{shown}");
    let mut found = false;
    for _ in 0..20 {
        app.scroll_delta = 1;
        if screen(40, 14, &mut app).contains("END") {
            found = true;
            break;
        }
    }
    assert!(found, "the command tail must remain reachable");
    assert_eq!(app.selected_command(), Some(command.as_str()));
}

#[test]
fn streaming_followup_never_reveals_an_older_selected_command() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("OLD_QUESTION");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "Old answer".into(),
        command: "printf OLD_COMMAND".into(),
    })
    .unwrap();
    app.editor.insert("CURRENT_QUESTION");
    app.begin().unwrap();
    app.delta(&format!(
        "{{\"explanation\":\"{}\\nTAIL_MARKER",
        "long answer words ".repeat(20)
    ))
    .unwrap();
    let shown = screen(40, 14, &mut app);
    assert!(shown.contains("CURRENT_QUESTION"), "{shown}");
    assert!(shown.contains("TAIL_MARKER"), "{shown}");
    assert!(!shown.contains("OLD_COMMAND"), "{shown}");
}

#[test]
fn simulation_is_visible_even_with_direct_start_context_summary() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), true);
    assert!(screen(80, 14, &mut app).contains("SIM"));
}

#[test]
fn explanations_render_commonmark_without_changing_command_bytes() {
    assert!(!ui::explanation_text("&#x1b;[31m &#x85;").contains('\u{1b}'));
    assert!(!ui::explanation_text("&#x1b;[31m &#x85;").contains('\u{85}'));
    assert_eq!(
        ui::explanation_text("Ja, `ls` zeigt **Ordner** und Dateien."),
        "Ja, ls zeigt Ordner und Dateien."
    );
    assert_eq!(
        ui::explanation_text("Pfad: `__init__.py` und `**/src/**`."),
        "Pfad: __init__.py und **/src/**."
    );
    assert_eq!(
        ui::explanation_text("Siehe [Doku](https://example.invalid/docs)."),
        "Siehe Doku (https://example.invalid/docs)."
    );
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("Test");
    app.begin().unwrap();
    let command = "printf '%s' '`ls` **literal**'";
    app.complete(Reply {
        explanation: "Ja, `ls` geht auch.".into(),
        command: command.into(),
    })
    .unwrap();
    let shown = screen(100, 14, &mut app);
    assert!(shown.contains("Ja, ls geht auch."));
    assert!(shown.contains(command));
    assert_eq!(app.selected_command(), Some(command));
}

#[test]
fn header_balances_identity_left_and_model_right_and_keeps_small_controls() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    let mut term = Terminal::new(TestBackend::new(160, 14)).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let b = term.backend().buffer();
    let top = (0..160).map(|x| b[(x, 0)].symbol()).collect::<String>();
    assert!(top[..top.find("amber").unwrap()].chars().count() < 15);
    assert!(top[..top.find("GPT-6 Luna").unwrap()].chars().count() > 100);
    assert!(top.trim_end().ends_with("Ctrl-O"));
    for width in [24, 40] {
        let shown = screen(width, 14, &mut app);
        assert!(shown.contains("Ctrl-O"));
        assert!(shown.contains("Ctrl-K"));
    }
}

#[test]
fn model_controls_fit_longer_catalog_labels_in_standard_and_fast_modes() {
    use amber_shell::provider::ModelInfo;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    for (id, label) in [("gpt-6-astra", "GPT-6-Astra"), ("gpt-6-luna", "GPT-6-Luna")] {
        app.model_id = id.into();
        app.models = vec![ModelInfo {
            id: id.into(),
            label: label.into(),
            supports_fast: true,
        }];
        for fast in [false, true] {
            app.fast = fast;
            for width in [24, 40, 65, 160] {
                let shown = screen(width, 14, &mut app);
                assert!(
                    shown.contains("Ctrl-O"),
                    "missing full shortcut: {width}, {id}, {fast}"
                );
                if fast && width >= 40 {
                    assert!(shown.contains("Fast"));
                }
            }
        }
    }
}

#[test]
fn explanations_wrap_at_words_while_commands_keep_exact_text() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("Test");
    app.begin().unwrap();
    let command = "printf '%s' 'abcdefghijklmnopqrstuvwx-long-word'";
    app.complete(Reply {
        explanation:
            "Führe die Tests des Rust-Projekts aus. Damit prüfst du die bestehenden Funktionen."
                .into(),
        command: command.into(),
    })
    .unwrap();
    let shown = screen(40, 20, &mut app);
    assert!(
        shown.contains("aus. Damit"),
        "words should move together onto the next display row"
    );
    assert_eq!(app.selected_command(), Some(command));
}

#[test]
fn tiny_header_keeps_status_and_simulation_badge_readable() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("Test");
    app.begin().unwrap();
    assert!(screen(24, 14, &mut app).contains("Waiting"));
    app.delta("{").unwrap();
    assert!(screen(24, 14, &mut app).contains("Stream"));
    app.complete(Reply {
        explanation: "Kurz".into(),
        command: "printf test".into(),
    })
    .unwrap();
    assert!(screen(24, 14, &mut app).contains("Done"));
    app.simulated = true;
    app.fast = true;
    for width in [24, 38, 40] {
        let shown = screen(width, 14, &mut app);
        assert!(shown.contains("SIM"));
        assert!(shown.contains("Ctrl-O"));
    }
}

#[test]
fn reopened_history_renders_both_sources_only_when_requested() {
    use amber_shell::model::Turn;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.past_turns.push(Turn {
        question: "alte Frage".into(),
        stream: String::new(),
        error: None,
        reply: Some(Reply {
            explanation: "alte Erklärung".into(),
            command: "printf saved".into(),
        }),
    });
    app.shell_commands.push("printf shell".into());
    let fresh = screen(100, 14, &mut app);
    assert!(!fresh.contains("alte Frage"));
    assert!(!fresh.contains("printf shell"));
    app.toggle_history();
    let history = screen(100, 14, &mut app);
    assert!(history.contains("Amber"));
    assert!(history.contains("alte Frage"));
    assert!(history.contains("printf saved"));
    app.select(-1);
    let shell = screen(100, 14, &mut app);
    assert!(shell.contains("Shell"));
    assert!(shell.contains("> printf shell"));
    assert_eq!(app.selected_command(), Some("printf shell"));
    app.toggle_history();
    let fresh = screen(100, 14, &mut app);
    assert!(!fresh.contains("alte Frage"));
    assert!(!fresh.contains("printf shell"));
}

#[test]
fn signet_uses_one_cell_on_one_line_in_every_state_and_fallback() {
    use unicode_width::UnicodeWidthStr;
    for ascii in [false, true] {
        for no_color in [false, true] {
            for state in 0..5 {
                let mut app = App::new(std::array::from_fn(|_| String::new()), false);
                app.ascii = ascii;
                app.no_color = no_color;
                if state > 0 {
                    app.editor.insert("Test");
                    app.begin().unwrap();
                }
                if state == 2 {
                    app.delta("{\"explanation\":\"Antwort").unwrap();
                }
                if state == 3 {
                    app.complete(Reply {
                        explanation: "Fertig".into(),
                        command: "printf test".into(),
                    })
                    .unwrap();
                }
                if state == 4 {
                    app.fail("Offline".into());
                }
                let mut marks = std::collections::HashSet::new();
                for frame in 0..4 {
                    app.indicator_frame = frame;
                    let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
                    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
                    let b = terminal.backend().buffer();
                    marks.insert(b[(1, 0)].symbol().to_owned());
                    assert_eq!(b[(1, 0)].symbol().width(), 1);
                    assert_ne!(b[(1, 0)].symbol(), " ");
                    for (x, y) in [(2, 0), (3, 0), (1, 1), (2, 1)] {
                        assert_eq!(b[(x, y)].symbol(), " ", "oversized signet {state}/{frame}");
                    }
                    assert_eq!(b[(4, 0)].symbol(), "a");
                    if ascii {
                        assert!(b[(1, 0)].symbol().is_ascii());
                    }
                    if no_color {
                        assert!(
                            b.content
                                .iter()
                                .all(|c| c.fg == ratatui::style::Color::Reset
                                    && c.bg == ratatui::style::Color::Reset)
                        );
                    }
                }
                if state == 1 || state == 2 {
                    assert!(
                        marks.len() > 1,
                        "active mark must animate independently of color"
                    );
                }
            }
        }
    }
}

#[test]
fn middle_grows_for_latest_answer_before_hiding_it_behind_old_turns() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("ALTE_FRAGE");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "Alter Beitrag".repeat(8),
        command: "printf old".into(),
    })
    .unwrap();
    app.editor.insert("AKTUELLE_FRAGE");
    app.begin().unwrap();
    let command = "printf first\nprintf second";
    app.complete(Reply {
        explanation:
            "Erste Zeile\nZweite Zeile\nDritte Zeile\nVierte Zeile\nFünfte Zeile\nSechste Zeile"
                .into(),
        command: command.into(),
    })
    .unwrap();
    let shown = screen(80, 20, &mut app);
    assert!(shown.contains("AKTUELLE_FRAGE"));
    assert!(shown.contains("Erste Zeile"));
    assert!(shown.contains("printf second"));
    assert!(!shown.contains("ALTE_FRAGE"));
    assert!(
        app.cursor_row >= 16,
        "middle should receive the additional inline height"
    );
    assert_eq!(app.selected_command(), Some(command));
}

#[test]
fn overlong_latest_answer_keeps_question_and_marks_hidden_answer_lines() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("AKTUELLE_FRAGE");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: (0..50).map(|i| format!("Erklärungszeile {i}\n")).collect(),
        command: "printf newest".into(),
    })
    .unwrap();
    let shown = screen(40, 18, &mut app);
    assert!(shown.contains("AKTUELLE_FRAGE"));
    assert!(shown.contains("earlier lines"));
    assert!(shown.contains("printf newest"));
    app.scroll_delta = -3;
    let manual = screen(40, 18, &mut app);
    assert!(
        !manual.contains("earlier lines"),
        "manual scroll must retain its own reading anchor"
    );
}

#[test]
fn automatic_follow_drops_partial_older_turns_as_a_whole() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.editor.insert("Wie prüfe ich dieses Projekt?");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "Starte zunächst die vorhandenen Tests.".into(),
        command: "printf OLD_COMMAND".into(),
    })
    .unwrap();
    app.editor
        .insert("Wie zeige ich dabei auch die Testausgaben?");
    app.begin().unwrap();
    app.delta("{\"explanation\":\"Du kannst die Ausgabe der Tests mit einer zusätzlichen Option einblenden.").unwrap();
    let shown = screen(40, 18, &mut app);
    assert!(shown.contains("Wie zeige ich dabei"));
    if shown.contains("OLD_COMMAND") {
        assert!(
            shown.contains("Wie prüfe ich dieses Projekt?"),
            "older commands may stay only with their complete question: {shown}"
        );
    }
}

#[test]
fn manual_reading_anchor_keeps_same_text_when_vertical_spacing_collapses() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    for i in 0..20 {
        app.editor.insert(&format!("QUESTION_{i}"));
        app.begin().unwrap();
        app.complete(Reply {
            explanation: format!("EXPLANATION_{i}"),
            command: format!("printf COMMAND_{i}"),
        })
        .unwrap();
    }
    app.command_focus = true;
    app.follow = false;
    app.anchor = 0;
    fn first_body(width: u16, height: u16, app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| ui::render(f, app)).unwrap();
        let y = if height >= 11 { 3 } else { 2 };
        (0..width)
            .map(|x| term.backend().buffer()[(x, y)].symbol())
            .collect()
    }
    let mut found = false;
    for _ in 0..200 {
        if first_body(80, 20, &mut app).contains("EXPLANATION_10") {
            found = true;
            break;
        }
        app.scroll_delta = 1;
    }
    assert!(found);
    let anchor = app.anchor;
    assert!(first_body(80, 10, &mut app).contains("EXPLANATION_10"));
    assert_eq!(app.anchor, anchor);
    assert!(first_body(80, 20, &mut app).contains("EXPLANATION_10"));
}

#[test]
fn sign_in_view_is_compact_and_hides_conversation_controls_and_private_state() {
    use amber_shell::model::SignInState;
    let mut app = App::new(std::array::from_fn(|_| "PRIVATE_CONTEXT".into()), false);
    app.editor.insert("PRIVATE_DRAFT");
    app.reviewing = true;
    app.model_picker = true;
    app.command_focus = true;
    app.shell_commands.push("PRIVATE_COMMAND".into());
    for (state, message, action) in [
        (SignInState::Required, "browser", "log in"),
        (SignInState::Starting, "Opening", ""),
        (SignInState::Waiting, "browser", "restart"),
        (
            SignInState::Failed("https://private.invalid/?token=SECRET".into()),
            "amber-shell doctor",
            "retry",
        ),
    ] {
        app.sign_in = Some(state);
        for (width, height) in [(100, 20), (40, 14), (24, 9), (24, 6)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| ui::render(f, &mut app)).unwrap();
            let b = terminal.backend().buffer();
            let rows: Vec<String> = (0..height)
                .map(|y| (0..width).map(|x| b[(x, y)].symbol()).collect())
                .collect();
            let text = rows.join("\n");
            assert!(text.contains("amber"), "{width}x{height}: {text}");
            assert!(
                text.contains("Log in with ChatGPT"),
                "{width}x{height}: {text}"
            );
            assert!(text.contains(message), "{width}x{height}: {text}");
            assert!(
                text.to_ascii_lowercase().contains(action),
                "{width}x{height}: {text}"
            );
            assert!(
                text.contains("Esc/Ctrl-C cancel"),
                "{width}x{height}: {text}"
            );
            for hidden in [
                "PRIVATE_", "SECRET", "https://", "Ctrl-O", "Ctrl-K", "Ctrl-Y", "Question",
            ] {
                assert!(
                    !text.contains(hidden),
                    "{width}x{height}: exposed {hidden}: {text}"
                );
            }
            assert_eq!(b[(1, 0)].symbol().chars().count(), 1);
            assert!(app.cursor_row < height);
        }
    }
    assert_eq!(app.editor.text, "PRIVATE_DRAFT");
    assert!(app.turns.is_empty());
}

#[test]
fn sign_in_explains_the_browser_and_private_amber_login_with_wrapped_text() {
    use amber_shell::model::SignInState;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.sign_in = Some(SignInState::Required);
    for (width, height) in [(40, 14), (24, 9), (24, 6)] {
        let shown = screen(width, height, &mut app);
        assert!(
            shown.contains("Private Amber login"),
            "{width}x{height}: {shown}"
        );
        assert!(shown.contains("Codex"), "{width}x{height}: {shown}");
        assert!(shown.contains("separate"), "{width}x{height}: {shown}");
        assert!(shown.contains("Enter"), "{width}x{height}: {shown}");
        assert!(shown.contains("browser"), "{width}x{height}: {shown}");
        assert!(!shown.contains('…'), "login copy must wrap: {shown}");
    }
    app.sign_in = Some(SignInState::Waiting);
    let shown = screen(40, 14, &mut app);
    assert!(shown.contains("automatically"), "{shown}");
}

#[test]
fn sign_in_branding_animates_without_color_and_uses_a_single_ascii_cell() {
    use amber_shell::model::SignInState;
    use ratatui::style::Color;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.sign_in = Some(SignInState::Waiting);
    app.no_color = true;
    app.ascii = true;
    app.reduced_motion = false;
    let mut marks = std::collections::HashSet::new();
    for indicator in 0..4 {
        app.indicator_frame = indicator;
        let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();
        terminal.draw(|f| ui::render(f, &mut app)).unwrap();
        let b = terminal.backend().buffer();
        marks.insert(b[(1, 0)].symbol().to_owned());
        assert!(b.content.iter().all(|c| c.symbol().is_ascii()));
        assert!(
            b.content
                .iter()
                .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
        );
        assert_eq!(b[(2, 0)].symbol(), " ");
        assert_eq!(b[(1, 1)].symbol(), " ");
    }
    assert!(marks.len() > 1);
}
