use amber_shell::capture::CaptureBuffer;
use amber_shell::model::App;

#[test]
fn captures_terminal_output_with_command_directory_and_status() {
    let mut capture = CaptureBuffer::default();
    capture.begin("ls missing", "/demo/project");
    capture.output(b"\x1b[31mls: missing: No such file or directory\x1b[0m\r\n");
    capture.finish(2);
    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].command, "ls missing");
    assert_eq!(records[0].cwd, "/demo/project");
    assert_eq!(records[0].status, 2);
    assert_eq!(
        records[0].output,
        "ls: missing: No such file or directory\n"
    );
    let mut app = App::from_shell_context_with_outputs(
        std::path::Path::new("/demo/project"),
        &["ls missing".into()],
        records,
        false,
    );
    assert!(app.selected_context().contains("No such file or directory"));
    assert!(app.selected_context().contains("Exit: 2"));
    app.set_context_choice(4);
    assert!(!app.selected_context().contains("No such file or directory"));
    assert!(!app.selected_context().contains("ls missing"));
}

#[test]
fn bounds_output_and_records_and_discards_unfinished_intervals() {
    let mut capture = CaptureBuffer::default();
    capture.output(b"prompt must not be recorded");
    for n in 0..8 {
        capture.begin(&format!("printf command-{n}"), "/demo");
        for _ in 0..300 {
            capture.output(b"large harmless output 0123456789\n");
        }
        capture.output(b"FINAL_ERROR_MARKER\n");
        capture.finish(1);
    }
    assert!(capture.records().len() > 3);
    for record in capture.records() {
        assert!(record.output.len() <= 4096);
        assert!(record.output.ends_with("FINAL_ERROR_MARKER\n"));
        assert!(record.truncated);
    }
    assert!(serde_json::to_vec(capture.records()).unwrap().len() <= 24 * 1024);
    capture.begin("unfinished", "/demo");
    capture.output(b"UNFINISHED_PRIVATE_OUTPUT");
    assert!(
        !serde_json::to_string(capture.records())
            .unwrap()
            .contains("UNFINISHED")
    );
}

#[test]
fn secrets_stay_excluded_even_after_the_header_has_left_the_tail() {
    let mut capture = CaptureBuffer::default();
    capture.begin("printf environment", "/demo");
    capture.output(b"API_");
    capture.output(b"KEY=synthetic-secret\n");
    for _ in 0..500 {
        capture.output(b"otherwise harmless output\n");
    }
    capture.finish(0);
    assert!(capture.records().is_empty());
    capture.begin("cat ~/.ssh/id_ed25519", "/demo");
    capture.output(b"bare sensitive contents without an identifying header");
    capture.finish(0);
    assert!(capture.records().is_empty());
}

#[test]
fn input_and_fullscreen_output_are_not_attached_as_normal_command_output() {
    let mut capture = CaptureBuffer::default();
    capture.begin("read -s answer", "/demo");
    capture.input(b"private typed answer\r");
    capture.output(b"echoed private typed answer");
    capture.finish(0);
    assert!(capture.records()[0].output.is_empty());
    assert!(capture.records()[0].omitted_reason.is_some());
    capture.begin("some interactive program", "/demo");
    capture.output(b"\x1b[?10");
    capture.output(b"49hprivate fullscreen contents\x1b[?1049l");
    capture.finish(0);
    assert!(capture.records()[1].output.is_empty());
    assert!(capture.records()[1].omitted_reason.is_some());
    capture.begin("sleep 10", "/demo");
    capture.input(b"\x03");
    capture.output(b"interrupted\n");
    capture.finish(130);
    assert!(capture.records()[2].output.contains("interrupted"));
}

#[test]
fn normalizes_carriage_return_updates_and_excludes_erased_prompt_markers() {
    let mut capture = CaptureBuffer::default();
    capture.begin("printf error", "/demo");
    capture.output(
        b"progress one\rprogress two\rfinal error\r\n\x1b[7m%\x1b[0m                  \r \r",
    );
    capture.finish(1);
    assert_eq!(capture.records()[0].output, "final error\n");
}

#[test]
fn relative_commands_in_sensitive_directories_are_excluded() {
    let mut capture = CaptureBuffer::default();
    for cwd in [
        "/Users/example/.ssh",
        "/Users/example/.aws",
        "/Users/example/.codex",
    ] {
        capture.begin("cat local-file", cwd);
        capture.output(b"unlabelled sensitive contents");
        capture.finish(0);
    }
    assert!(capture.records().is_empty());
}

#[test]
fn small_outputs_keep_more_than_three_steps_without_growing_the_snapshot() {
    let mut capture = CaptureBuffer::default();
    for n in 0..12 {
        capture.begin(&format!("printf step-{n}"), "/demo");
        capture.output(format!("result-{n}\n").as_bytes());
        capture.finish(0);
    }
    assert_eq!(capture.records().len(), 12);
    let app = App::from_shell_context_with_outputs(
        std::path::Path::new("/demo"),
        &[],
        capture.records(),
        false,
    );
    assert!(app.selected_context().contains("result-0"));
    assert!(app.selected_context().contains("result-11"));
    for n in 12..500 {
        capture.begin(&format!("printf step-{n}"), "/demo");
        capture.output(b"small output\n");
        capture.finish(0);
    }
    assert!(serde_json::to_vec(capture.records()).unwrap().len() <= 24 * 1024);
    assert_eq!(capture.records().last().unwrap().command, "printf step-499");
}

#[test]
fn send_budget_shortens_large_tails_instead_of_dropping_the_whole_error_chain() {
    let mut capture = CaptureBuffer::default();
    for n in 0..5 {
        capture.begin(&format!("printf step-{n}"), "/demo");
        capture.output(&vec![b'x'; 4096]);
        capture.output(format!("\nERROR_STEP_{n}\n").as_bytes());
        capture.finish(1);
    }
    let app = App::from_shell_context_with_outputs(
        std::path::Path::new("/demo"),
        &[],
        capture.records(),
        false,
    );
    for n in 0..5 {
        assert!(app.selected_context().contains(&format!("ERROR_STEP_{n}")));
    }
    assert!(serde_json::to_string(app.selected_context()).unwrap().len() < 14 * 1024);
    assert!(app.selected_context().contains("Gekürzt"));
}
