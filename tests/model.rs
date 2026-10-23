use amber_shell::model::validate_command;
use amber_shell::model::{App, Editor, Reply};

#[test]
fn commands_reject_empty_controls_and_oversize_without_changing_allowed_text() {
    for bad in [
        "",
        " \n\t",
        "echo\0x",
        "echo\u{1b}[31m",
        "echo\r",
        "echo\u{85}",
        "echo\u{7f}",
    ] {
        assert!(validate_command(bad).is_err(), "accepted {bad:?}");
    }
    assert!(validate_command(&"x".repeat(16385)).is_err());
    assert!(validate_command(&"x".repeat(16384)).is_ok());
    let original = "  printf '%s\\n' 'Grüße e\u{301} 👩‍💻 漢字'\n\tprintf 'done' \n";
    assert!(validate_command(original).is_ok());
    assert!(original.ends_with(" \n"));
}

#[test]
fn editor_moves_and_deletes_whole_graphemes_and_preserves_paste() {
    let mut edit = Editor::default();
    edit.insert("a👩‍💻e\u{301}z");
    edit.left();
    edit.backspace();
    assert_eq!(edit.text, "a👩‍💻z");
    edit.left();
    edit.delete();
    assert_eq!(edit.text, "az");
    edit.insert("\n漢字");
    assert_eq!(edit.text, "a\n漢字z");
    edit.home();
    edit.right();
    assert_eq!(edit.cursor, 1);
}

#[test]
fn inserting_a_joiner_cannot_leave_cursor_inside_a_grapheme() {
    let mut editor = Editor::default();
    editor.insert("👩💻");
    editor.left();
    editor.insert("\u{200d}");
    assert_eq!(editor.text, "👩‍💻");
    editor.backspace();
    assert_eq!(editor.text, "");
}

#[test]
fn session_keeps_draft_and_completed_selection_across_request_failures() {
    let mut app = App::new(
        [
            "cwd".into(),
            "files".into(),
            "history".into(),
            "both".into(),
        ],
        false,
    );
    app.editor.insert("erste Frage");
    let (q, conv) = app.begin().unwrap();
    assert_eq!(q, "erste Frage");
    assert!(conv.is_empty());
    assert!(app.begin().is_err());
    app.complete(Reply {
        explanation: "kurz".into(),
        command: " printf 'ok'\n".into(),
    })
    .unwrap();
    assert_eq!(app.selected_command(), Some(" printf 'ok'\n"));
    app.editor.insert("Rückfrage");
    let (_, conv) = app.begin().unwrap();
    assert_eq!(conv.len(), 1);
    app.editor.insert("noch nicht senden");
    app.fail("offline".into());
    assert_eq!(app.editor.text, "noch nicht senden");
    assert_eq!(app.selected_command(), Some(" printf 'ok'\n"));
    assert!(!app.active);
}

#[test]
fn answer_indicator_tracks_real_state_and_stops_after_completion_or_failure() {
    use amber_shell::model::AnswerStatus;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.reduced_motion = false;
    assert_eq!(app.answer_status(), AnswerStatus::Ready);
    assert!(!app.advance_indicator());
    app.editor.insert("Test");
    app.begin().unwrap();
    assert_eq!(app.answer_status(), AnswerStatus::Waiting);
    assert!(app.advance_indicator());
    assert_eq!(app.indicator_frame, 1);
    app.delta("{").unwrap();
    assert_eq!(app.answer_status(), AnswerStatus::Streaming);
    app.complete(Reply {
        explanation: "Test".into(),
        command: "printf test".into(),
    })
    .unwrap();
    assert_eq!(app.answer_status(), AnswerStatus::Complete);
    assert!(!app.advance_indicator());
    app.editor.insert("Followup");
    app.begin().unwrap();
    assert_eq!(app.indicator_frame, 0);
    app.reduced_motion = true;
    assert!(!app.advance_indicator());
    app.fail("offline".into());
    assert_eq!(app.answer_status(), AnswerStatus::Error);
    assert!(!app.advance_indicator());
}

#[test]
fn startup_includes_recent_commands_and_context_can_be_disabled() {
    let mut app = App::new(
        [
            "folder".into(),
            "files".into(),
            "history".into(),
            "both".into(),
        ],
        false,
    );
    assert!(!app.reviewing);
    assert!(!app.command_focus);
    assert_eq!(app.context_choice, 3);
    assert_eq!(app.selected_context(), "both");
    app.context_choice = 4;
    assert_eq!(app.selected_context(), "");
    assert!(app.context_preview().contains("No new context"));
}

#[test]
fn disabling_new_context_preserves_followup_history_and_explains_that_limit() {
    let mut app = App::new(std::array::from_fn(|_| "context".into()), false);
    app.editor.insert("erste Frage");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "vorheriger Dateikontext".into(),
        command: "printf previous".into(),
    })
    .unwrap();
    app.context_choice = 4;
    app.editor.insert("Rückfrage");
    let (_, history) = app.begin().unwrap();
    assert!(app.selected_context().is_empty());
    assert!(history[0].1.contains("vorheriger Dateikontext"));
    assert!(
        app.context_preview()
            .contains("Earlier questions and answers stay")
    );
}

#[test]
fn reopened_history_exposes_amber_and_shell_without_sending_them_as_conversation() {
    use amber_shell::model::Turn;
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.past_turns = vec![Turn {
        question: "früher".into(),
        stream: String::new(),
        reply: Some(Reply {
            explanation: "alte Erklärung".into(),
            command: "printf previous".into(),
        }),
        error: None,
    }];
    app.shell_commands = vec!["printf shell".into()];
    app.toggle_history();
    assert_eq!(app.selected_command(), Some("printf previous"));
    app.select(-1);
    assert_eq!(app.selected_command(), Some("printf shell"));
    app.toggle_history();
    app.editor.insert("neue Frage");
    let (_, conversation) = app.begin().unwrap();
    assert!(
        conversation.is_empty(),
        "local browsing history must not silently join a new request"
    );
    app.complete(Reply {
        explanation: "neu".into(),
        command: "printf new".into(),
    })
    .unwrap();
    app.toggle_history();
    app.select(10);
    assert_eq!(app.selected_command(), Some("printf new"));
}

#[test]
fn returning_to_question_cannot_transfer_a_hidden_old_selection() {
    let mut app = App::new(std::array::from_fn(|_| String::new()), false);
    app.shell_commands = vec!["printf old".into()];
    app.toggle_history();
    assert_eq!(app.selected_command(), Some("printf old"));
    app.select(0);
    app.toggle_history();
    assert_eq!(app.selected_command(), None);
    app.editor.insert("neue Frage");
    app.begin().unwrap();
    app.complete(Reply {
        explanation: "sichtbar".into(),
        command: "printf new".into(),
    })
    .unwrap();
    assert_eq!(app.selected_command(), Some("printf new"));
    app.toggle_history();
    app.select(-1);
    assert_eq!(app.selected_command(), Some("printf old"));
    app.toggle_history();
    assert_eq!(app.selected_command(), Some("printf new"));
}

#[test]
fn terminal_mode_does_not_load_files_and_automatic_cache_is_reused_in_a_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("package.json");
    std::fs::write(&manifest, r#"{"scripts":{"dev":"first-marker"}}"#).unwrap();
    let mut app = App::from_shell_context(
        dir.path(),
        &["pwd".into(), "API_KEY=synthetic-secret".into()],
        false,
    );
    app.set_context_choice(2);
    app.editor.insert("npm dev?");
    app.begin().unwrap();
    assert!(!app.selected_context().contains("first-marker"));
    assert!(app.selected_context().contains("pwd"));
    assert!(!app.selected_context().contains("synthetic-secret"));
    app.fail("offline".into());
    std::fs::write(&manifest, r#"{"scripts":{"dev":"read-on-question"}}"#).unwrap();
    app.set_context_choice(3);
    app.editor.insert("npm dev?");
    app.begin().unwrap();
    assert!(app.selected_context().contains("read-on-question"));
    app.fail("offline".into());
    std::fs::write(&manifest, r#"{"scripts":{"dev":"must-not-reread"}}"#).unwrap();
    app.set_context_choice(2);
    app.set_context_choice(3);
    app.editor.insert("npm dev?");
    app.begin().unwrap();
    assert!(app.selected_context().contains("read-on-question"));
    assert!(!app.selected_context().contains("must-not-reread"));
}

#[test]
fn automatic_context_selects_compact_nested_npm_scripts_only_for_project_questions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("apps/web")).unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("apps/web/package.json"),
        r#"{"name":"@demo/web","scripts":{"dev":"vite --host","build":"vite build"},"description":"OMIT_DESCRIPTION","dependencies":{"vite":"1.0.0"}}"#).unwrap();
    let mut app = App::from_shell_context(dir.path(), &["pwd".into()], false);
    assert!(!app.selected_context().contains("vite"));
    app.editor.insert("Wie liste ich Ordner auf?");
    app.begin().unwrap();
    assert!(!app.selected_context().contains("package.json"));
    app.fail("offline".into());
    app.editor.insert("Wie starte ich das npm Web Projekt?");
    app.begin().unwrap();
    let context = app.selected_context();
    assert!(context.contains("apps/web/package.json"), "{context}");
    assert!(context.contains("vite --host"));
    assert!(context.contains("pnpm@10.0.0"));
    assert!(!context.contains("OMIT_DESCRIPTION"));
    assert!(context.len() < 4096);
}
