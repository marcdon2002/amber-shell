use crate::model::{AnswerStatus, App, Turn, safe_text};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// Offsets belong to the unwrapped content, so width changes do not move the reading anchor.
struct Row {
    text: String,
    offset: usize,
}
enum ContentKind {
    Spacing,
    Question,
    Explanation,
    Command { selected: bool },
}
struct ContentSpan {
    range: std::ops::Range<usize>,
    kind: ContentKind,
    continuation: &'static str,
    grouped: bool,
}
fn wrap(text: &str, width: usize) -> Vec<Row> {
    wrap_content(text, width, &[])
}
fn content_rows(text: &str, width: usize, spans: &[ContentSpan], roomy: bool) -> Vec<Row> {
    wrap_content(text, width, spans)
        .into_iter()
        .filter(|row| {
            roomy
                || !spans.iter().any(|s| {
                    matches!(s.kind, ContentKind::Spacing) && s.range.contains(&row.offset)
                })
        })
        .collect()
}
fn append_spacing(content: &mut String, spans: &mut Vec<ContentSpan>) {
    let start = content.len();
    content.push('\n');
    spans.push(ContentSpan {
        range: start..content.len(),
        kind: ContentKind::Spacing,
        continuation: "",
        grouped: false,
    });
}
// Only UI labels pass here. Commands, explanations and input keep their original text.
fn chrome(text: &str, ascii: bool) -> String {
    if !ascii {
        return text.into();
    }
    text.replace('·', "|")
        .replace('↑', "^")
        .replace('↓', "v")
        .replace('←', "<")
        .replace('→', ">")
        .replace('…', "...")
        .replace('×', "x")
}
fn wrap_content(text: &str, width: usize, spans: &[ContentSpan]) -> Vec<Row> {
    if width == 0 {
        return vec![];
    }
    let mut rows = vec![];
    let mut line = String::new();
    let mut cells = 0;
    let mut start = 0;
    for (offset, g) in text.grapheme_indices(true) {
        if g == "\n" {
            rows.push(Row {
                text: std::mem::take(&mut line),
                offset: start,
            });
            cells = 0;
            start = offset + 1;
            continue;
        }
        let span = spans.iter().find(|c| c.range.contains(&offset));
        if line.is_empty() && span.is_some_and(|c| offset > c.range.start) && width > 2 {
            let prefix = span.unwrap().continuation;
            line.push_str(prefix);
            cells = prefix.width();
        }
        // Prose keeps whole words when they fit; commands retain character-exact wrapping.
        if let Some(span) =
            span.filter(|s| matches!(s.kind, ContentKind::Question | ContentKind::Explanation))
        {
            let word_start = !g.chars().all(char::is_whitespace)
                && (offset == 0
                    || text[..offset]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace));
            if word_start {
                let word = text[offset..]
                    .split(char::is_whitespace)
                    .next()
                    .unwrap_or("");
                let indent = span.continuation.width();
                if cells > indent
                    && cells + word.width() > width
                    && word.width() <= width.saturating_sub(indent)
                {
                    rows.push(Row {
                        text: line.trim_end().to_owned(),
                        offset: start,
                    });
                    line = span.continuation.to_owned();
                    cells = indent;
                    start = offset;
                }
            }
        }
        let display = if g == "\t" { "    " } else { g };
        let len = display.width();
        if cells + len > width && !line.is_empty() {
            rows.push(Row {
                text: std::mem::take(&mut line),
                offset: start,
            });
            if let Some(span) = span.filter(|_| width > 4) {
                line.push_str(span.continuation);
                cells = span.continuation.width();
            } else {
                cells = 0;
            }
            start = offset;
            if span
                .is_some_and(|s| matches!(s.kind, ContentKind::Question | ContentKind::Explanation))
                && g.chars().all(char::is_whitespace)
            {
                start = offset + g.len();
                continue;
            }
        }
        if len <= width {
            line.push_str(display);
            cells += len;
        }
    }
    rows.push(Row {
        text: line,
        offset: start,
    });
    rows
}

fn fit_label(text: &str, width: usize, ascii: bool) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut cells = 0;
    for g in text.graphemes(true) {
        if cells + g.width() > width - 1 {
            break;
        }
        out.push_str(g);
        cells += g.width();
    }
    out.push(if ascii { '~' } else { '…' });
    out
}

/// Decode explanatory Markdown as terminal-safe text; command bytes never enter this path.
pub fn explanation_text(raw: &str) -> String {
    let clean = safe_text(raw);
    let mut out = String::new();
    let mut links = Vec::new();
    for event in Parser::new(&clean) {
        match event {
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                out.push_str(&t)
            }
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Link { dest_url, .. }) => {
                links.push((out.len(), dest_url.into_string()))
            }
            Event::End(TagEnd::Link) => {
                if let Some((start, url)) = links.pop()
                    && out[start..] != url
                {
                    out.push_str(&format!(" ({url})"));
                }
            }
            Event::Start(Tag::Item) => out.push_str("- "),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock,
            ) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Event::Rule => out.push('\n'),
            _ => {}
        }
    }
    safe_text(out.trim_end_matches('\n'))
}

// Show the readable explanation while a JSON string is still streaming.
pub fn streaming_text(raw: &str) -> String {
    let Some(key) = raw.find("\"explanation\"") else {
        return "Composing answer ...".into();
    };
    let Some(colon) = raw[key + 13..].find(':') else {
        return String::new();
    };
    let rest = raw[key + 13 + colon + 1..].trim_start();
    let Some(rest) = rest.strip_prefix('"') else {
        return String::new();
    };
    let mut result = String::new();
    let mut escape = false;
    for c in rest.chars() {
        if escape {
            match c {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                '"' | '\\' => result.push(c),
                _ => {}
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '"' {
            break;
        } else {
            result.push(c);
        }
    }
    safe_text(&result)
}

fn append_command(
    content: &mut String,
    spans: &mut Vec<ContentSpan>,
    selected_offset: &mut Option<usize>,
    command: &str,
    selected: bool,
    ascii: bool,
    grouped: bool,
) {
    if selected {
        *selected_offset = Some(content.len());
    }
    let start = content.len();
    let (prefix, continuation) = if grouped {
        if !selected {
            if ascii {
                ("|          ", "|          ")
            } else {
                ("│          ", "│          ")
            }
        } else if ascii {
            ("|        > ", "|          ")
        } else {
            ("│        > ", "│          ")
        }
    } else if selected {
        ("> ", if ascii { "| " } else { "│ " })
    } else if ascii {
        ("| ", "| ")
    } else {
        ("│ ", "│ ")
    };
    content.push_str(prefix);
    content.push_str(&safe_text(command));
    spans.push(ContentSpan {
        range: start..content.len(),
        kind: ContentKind::Command { selected },
        continuation,
        grouped,
    });
    content.push('\n');
}

fn append_turn(
    content: &mut String,
    spans: &mut Vec<ContentSpan>,
    selected_offset: &mut Option<usize>,
    turn: &Turn,
    selected: bool,
    ascii: bool,
) {
    let rail = if ascii { "| " } else { "│ " };
    let continuation = if ascii { "|        " } else { "│        " };
    let start = content.len();
    content.push_str(&format!("{rail}You:   {}\n", safe_text(&turn.question)));
    spans.push(ContentSpan {
        range: start..content.len(),
        kind: ContentKind::Question,
        continuation,
        grouped: true,
    });
    let start = content.len();
    content.push_str(rail);
    content.push_str("Amber: ");
    if let Some(reply) = &turn.reply {
        content.push_str(&explanation_text(&reply.explanation));
    } else if let Some(error) = &turn.error {
        content.push_str(&format!("Error: {}", safe_text(error)));
    } else {
        content.push_str(&explanation_text(&streaming_text(&turn.stream)));
    }
    spans.push(ContentSpan {
        range: start..content.len(),
        kind: ContentKind::Explanation,
        continuation,
        grouped: true,
    });
    content.push('\n');
    if let Some(reply) = &turn.reply {
        append_command(
            content,
            spans,
            selected_offset,
            &reply.command,
            selected,
            ascii,
            true,
        );
    }
    append_spacing(content, spans);
}

fn row_line(
    row: &Row,
    spans: &[ContentSpan],
    base: Style,
    muted: Style,
    amber: Style,
) -> Line<'static> {
    let span = spans.iter().find(|s| s.range.contains(&row.offset));
    let style = match span.map(|s| &s.kind) {
        Some(ContentKind::Command { selected: true }) => amber,
        Some(ContentKind::Command { selected: false } | ContentKind::Question) => {
            base.add_modifier(Modifier::BOLD)
        }
        _ => base,
    };
    if span.is_some_and(|s| s.grouped) {
        // Fixed cell columns: rail, seven-cell role gutter, then shared body edge.
        let rail_end = row
            .text
            .char_indices()
            .nth(2)
            .map_or(row.text.len(), |(n, _)| n);
        let rest = &row.text[rail_end..];
        let role_end = rest.char_indices().nth(7).map_or(rest.len(), |(n, _)| n);
        let role_style = match span.map(|s| &s.kind) {
            Some(ContentKind::Explanation) => amber,
            Some(ContentKind::Question) => base.add_modifier(Modifier::BOLD),
            _ => muted,
        };
        Line::from(vec![
            Span::styled(row.text[..rail_end].to_owned(), muted),
            Span::styled(rest[..role_end].to_owned(), role_style),
            Span::styled(rest[role_end..].to_owned(), style),
        ])
    } else {
        Line::styled(row.text.clone(), style)
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let mut area = frame.area();
    if area.width < 24 || area.height < 6 {
        frame.render_widget(Paragraph::new("Enlarge window | Esc back"), area);
        frame.set_cursor_position((area.x, area.y));
        app.cursor_row = area.y;
        return;
    }
    let mut content = String::new();
    let mut selected_offset = None;
    let mut latest_question_offset = None;
    let mut spans = vec![];
    let roomy = area.height >= 11;
    if app.reviewing {
        content = app.context_preview();
    } else {
        if app.command_focus {
            if !app.shell_commands.is_empty() {
                content.push_str(&chrome("Shell · recent commands\n", app.ascii));
                for (i, command) in app.shell_commands.iter().enumerate() {
                    append_command(
                        &mut content,
                        &mut spans,
                        &mut selected_offset,
                        command,
                        app.selected == Some(i),
                        app.ascii,
                        false,
                    );
                }
                content.push('\n');
            }
            if !app.past_turns.is_empty() {
                content.push_str(&chrome("Amber · earlier in this folder\n", app.ascii));
                for (i, turn) in app.past_turns.iter().enumerate() {
                    append_turn(
                        &mut content,
                        &mut spans,
                        &mut selected_offset,
                        turn,
                        app.selected == Some(app.shell_commands.len() + i),
                        app.ascii,
                    );
                }
            }
            if !app.turns.is_empty() {
                content.push_str("Current conversation\n");
            }
            if app.history_empty() {
                content.push_str("No history in this folder yet.\nTab returns to your question.");
            }
        }
        if !app.command_focus && app.turns.is_empty() {
            content.push_str("Ask for a command.\nFor example: Which ports are open?");
        }
        for (i, turn) in app.turns.iter().enumerate() {
            latest_question_offset = Some(content.len());
            append_turn(
                &mut content,
                &mut spans,
                &mut selected_offset,
                turn,
                app.selected == Some(app.history_base() + i),
                app.ascii,
            );
        }
    }
    if !app.reviewing && !app.model_picker {
        let lines = content_rows(
            content.trim_end(),
            area.width.saturating_sub(2) as usize,
            &spans,
            roomy,
        )
        .len();
        area.height = area
            .height
            .min(lines.saturating_add(8).clamp(10, 20) as u16);
    }
    let bounds = area;
    area.x += 1;
    area.width = area.width.saturating_sub(2);
    let indexed = std::env::var("TERM")
        .unwrap_or_default()
        .contains("256color");
    let base = if app.no_color {
        Style::default()
    } else {
        Style::default().fg(if indexed {
            Color::Indexed(252)
        } else {
            Color::White
        })
    };
    let muted = if app.no_color {
        base
    } else {
        base.fg(if indexed {
            Color::Indexed(248)
        } else {
            Color::Gray
        })
    };
    let amber = if app.no_color {
        base.add_modifier(Modifier::BOLD)
    } else {
        base.fg(if indexed {
            Color::Indexed(223)
        } else {
            Color::Yellow
        })
        .add_modifier(Modifier::BOLD)
    };
    // Inherit the terminal background; only the current action receives an accent.
    frame.render_widget(Paragraph::new("").style(base), bounds);
    // A one-cell amber signet and left status balance the right-aligned settings.
    let status = app.answer_status();
    let compact_header = area.width < 36;
    let brand_col = 2;
    let model_col = if compact_header {
        8
    } else if app.simulated {
        area.width.saturating_sub(46).max(18)
    } else {
        area.width.saturating_sub(46).max(14)
    };
    let header_width = area.width;
    let model_width = header_width.saturating_sub(model_col);
    let label = safe_text(&app.model_label());
    let suffix = label.split([' ', '-']).next_back().unwrap_or(&label);
    let short = if let Some(c) = suffix.chars().next() {
        c.to_uppercase().to_string() + &suffix[c.len_utf8()..]
    } else {
        label.clone()
    };
    let tier = if app.fast { "Fast" } else { "Std" };
    let suffix = if compact_header {
        " Ctrl-O".to_owned()
    } else if area.width < 65 {
        format!(" {tier} Ctrl-O")
    } else {
        format!(
            " · {:8} · Ctrl-O",
            if app.fast { "Fast" } else { "Standard" }
        )
    };
    let name = if area.width < 65 { &short } else { &label };
    let model = format!(
        "{}{suffix}",
        fit_label(
            name,
            (model_width as usize).saturating_sub(suffix.width()),
            app.ascii
        )
    );
    let signet = if app.ascii {
        match status {
            AnswerStatus::Ready => ".",
            AnswerStatus::Waiting => [".", "o", "O", "o"][app.indicator_frame],
            AnswerStatus::Streaming => ["+", "*", "+", "."][app.indicator_frame],
            AnswerStatus::Complete => "*",
            AnswerStatus::Error => "!",
        }
    } else {
        match status {
            AnswerStatus::Ready => "◇",
            AnswerStatus::Waiting => ["◇", "◈", "◆", "◈"][app.indicator_frame],
            AnswerStatus::Streaming => ["◈", "◆", "◈", "◇"][app.indicator_frame],
            AnswerStatus::Complete => "◆",
            AnswerStatus::Error => "!",
        }
    };
    let signet_style = if app.active && !app.no_color {
        if indexed {
            amber.fg(Color::Indexed([179, 180, 222, 180][app.indicator_frame]))
        } else if app.indicator_frame == 0 || app.indicator_frame == 3 {
            amber.add_modifier(Modifier::DIM)
        } else {
            amber
        }
    } else {
        amber
    };
    // One glyph on one line: state changes never alter the mark's footprint.
    frame.render_widget(
        Paragraph::new(signet).style(signet_style),
        Rect::new(area.x, area.y, 1, 1),
    );
    let logo_label = if app.simulated {
        if compact_header { "SIM" } else { " amber SIM " }
    } else if compact_header {
        "amber"
    } else {
        " amber "
    };
    let logo_style = if app.no_color {
        base.add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        amber.fg(Color::Black).bg(if indexed {
            Color::Indexed(223)
        } else {
            Color::Yellow
        })
    };
    frame.render_widget(
        Paragraph::new(logo_label).style(logo_style),
        Rect::new(
            area.x + brand_col,
            area.y,
            (logo_label.width() as u16).min(model_col - brand_col - 1),
            1,
        ),
    );
    frame.render_widget(
        Paragraph::new(chrome(&model, app.ascii))
            .alignment(Alignment::Right)
            .style(if app.model_picker { amber } else { muted }),
        Rect::new(area.x + model_col, area.y, model_width, 1),
    );
    let notice = !app.status.is_empty() && !app.active && !app.status.starts_with("Ready");
    let state = if app.model_picker {
        if compact_header {
            "Models"
        } else {
            "Choose model"
        }
    } else if app.reviewing {
        "Context"
    } else if notice && status != AnswerStatus::Error {
        "Notice"
    } else if app.command_focus {
        "History"
    } else {
        match status {
            AnswerStatus::Ready => "Ready",
            AnswerStatus::Waiting => "Waiting",
            AnswerStatus::Streaming => {
                if compact_header {
                    "Stream"
                } else {
                    "Streaming"
                }
            }
            AnswerStatus::Complete => "Done",
            AnswerStatus::Error => "Error",
        }
    };
    let metadata_col = if compact_header {
        13
    } else {
        model_col.max(23)
    };
    let metadata_width = header_width.saturating_sub(metadata_col);
    frame.render_widget(
        Paragraph::new(state).style(if app.active { amber } else { muted }),
        Rect::new(
            area.x + brand_col + u16::from(!compact_header),
            area.y + 1,
            metadata_col - brand_col - 2 * u16::from(!compact_header),
            1,
        ),
    );
    let metadata = if app.model_picker {
        if metadata_width < 20 {
            "↑↓ Model".into()
        } else {
            "↑↓ Model · Ctrl-F Fast".into()
        }
    } else if app.reviewing {
        app.context_mode().label().into()
    } else if app.command_focus {
        format!(
            "{} Amber · {} Shell",
            app.past_turns.len() + app.turns.iter().filter(|t| t.reply.is_some()).count(),
            app.shell_commands.len()
        )
    } else if notice || app.active && app.status.starts_with("Model change") {
        safe_text(&app.status)
    } else if metadata_width < 14 {
        "Ctrl-K".into()
    } else if metadata_width < 36 {
        "Ctrl-K Context".into()
    } else {
        app.context_summary(metadata_width)
    };
    frame.render_widget(
        Paragraph::new(fit_label(
            &chrome(&metadata, app.ascii),
            metadata_width as usize,
            app.ascii,
        ))
        .alignment(Alignment::Right)
        .style(muted),
        Rect::new(area.x + metadata_col, area.y + 1, metadata_width, 1),
    );
    let composer = !app.reviewing && !app.model_picker && area.height >= 9;
    let top_gap = u16::from(area.height >= 11);
    let composer_gap = u16::from(composer && area.height >= 12);
    let bottom_rows = if composer { 4 + composer_gap } else { 2 };
    let body = Rect::new(
        area.x,
        area.y + 2 + top_gap,
        area.width,
        area.height - 2 - top_gap - bottom_rows,
    );
    let composer_rect = Rect::new(area.x, area.bottom() - 4, area.width, 3);
    let input = if composer {
        Rect::new(
            area.x + 2,
            area.bottom() - 3,
            area.width.saturating_sub(4),
            1,
        )
    } else {
        Rect::new(area.x, area.bottom() - 2, area.width, 1)
    };
    let hints = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    frame.set_cursor_position((input.x, input.y));
    app.cursor_row = input.y;
    if app.model_picker {
        let lines: Vec<Line> = if app.model_loading {
            vec![Line::styled("Loading available models …", muted)]
        } else if !app.model_error.is_empty() {
            wrap(&safe_text(&app.model_error), body.width as usize)
                .into_iter()
                .map(|r| Line::styled(r.text, base))
                .collect()
        } else {
            let top = app
                .model_cursor
                .saturating_sub(body.height.saturating_sub(1) as usize);
            app.models
                .iter()
                .enumerate()
                .skip(top)
                .take(body.height as usize)
                .map(|(i, m)| {
                    Line::styled(
                        format!(
                            "{} {}{}",
                            if i == app.model_cursor { ">" } else { " " },
                            safe_text(&m.label),
                            if m.id == app.model_id {
                                "  (active)"
                            } else {
                                ""
                            }
                        ),
                        if i == app.model_cursor { amber } else { base },
                    )
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), body);
        frame.render_widget(
            Paragraph::new(chrome(
                if app
                    .models
                    .get(app.model_cursor)
                    .is_some_and(|m| m.supports_fast)
                {
                    if app.picker_fast {
                        "Ctrl-F Fast on · Plan 2.5×/Credits 2×"
                    } else {
                        "Ctrl-F Fast off · Plan 2.5×/Credits 2×"
                    }
                } else {
                    "Fast unavailable for this model"
                },
                app.ascii,
            ))
            .style(muted),
            input,
        );
        frame.render_widget(
            Paragraph::new(chrome(
                if app.model_error.is_empty() {
                    "↑↓ select · Enter save · Esc"
                } else {
                    "Ctrl-O reload · Esc back"
                },
                app.ascii,
            ))
            .style(muted),
            hints,
        );
        return;
    }
    let rows = content_rows(
        safe_text(&content).trim_end(),
        body.width as usize,
        &spans,
        roomy,
    );
    let automatic_follow = !app.selection_reveal
        && app.follow
        && !app.reviewing
        && !app.command_focus
        && app.scroll_delta == 0;
    let max = rows.len().saturating_sub(body.height as usize);
    if app.selection_reveal {
        if let Some(offset) = selected_offset {
            app.anchor = offset;
            app.follow = false;
        }
        app.selection_reveal = false;
    }
    let mut top = if app.follow && !app.reviewing {
        if app.scroll_delta != 0 {
            app.last_top.min(max)
        } else {
            max
        }
    } else {
        rows.iter()
            .rposition(|r| r.offset <= app.anchor)
            .unwrap_or(0)
            .min(max)
    };
    if app.scroll_delta != 0 {
        top = (top as i64 + app.scroll_delta as i64).clamp(0, max as i64) as usize;
        app.anchor = rows.get(top).map_or(0, |r| r.offset);
        app.follow = top == max && !app.reviewing;
        app.scroll_delta = 0;
    }
    // Keep older turns only when their question fits too; detached tails add visual noise.
    if automatic_follow
        && top > 0
        && let Some(next) = rows.iter().enumerate().skip(top).find_map(|(i, r)| {
            spans
                .iter()
                .any(|s| s.range.start == r.offset && matches!(s.kind, ContentKind::Question))
                .then_some(i)
        })
    {
        top = next;
    }
    let pinned = latest_question_offset.and_then(|offset| {
        let first = rows.iter().position(|r| r.offset == offset)?;
        if !automatic_follow || body.height < 5 || first >= top {
            return None;
        }
        let question = spans
            .iter()
            .find(|s| s.range.start == offset && matches!(s.kind, ContentKind::Question))?;
        let count = rows[first..]
            .iter()
            .take_while(|r| question.range.contains(&r.offset))
            .count()
            .min(2);
        Some((first, count))
    });
    let mut lines = vec![];
    let mut visible_height = body.height as usize;
    if let Some((first, count)) = pinned {
        for r in &rows[first..first + count] {
            lines.push(row_line(r, &spans, base, muted, amber));
        }
        lines.push(Line::styled(
            if app.ascii {
                "|        ^ earlier lines"
            } else {
                "│        ↑ earlier lines"
            },
            muted,
        ));
        visible_height -= count + 1;
        top = rows.len().saturating_sub(visible_height);
    }
    // A wide role gutter must not turn the selected command into an unexplained tail.
    let mut command_continues = false;
    if automatic_follow
        && let Some(first) = selected_offset
            .filter(|offset| latest_question_offset.is_some_and(|question| *offset >= question))
            .and_then(|offset| rows.iter().position(|r| r.offset == offset))
        && first < top
    {
        top = first;
        command_continues = top + visible_height < rows.len() && visible_height > 1;
        visible_height -= usize::from(command_continues);
    }
    app.last_top = top;
    lines.extend(
        rows.iter()
            .skip(top)
            .take(visible_height)
            .map(|r| row_line(r, &spans, base, muted, amber)),
    );
    if command_continues {
        lines.push(Line::styled(
            if app.ascii {
                "|        v more"
            } else {
                "│        ↓ more"
            },
            muted,
        ));
    }
    frame.render_widget(Paragraph::new(lines), body);
    if app.reviewing {
        frame.render_widget(
            Paragraph::new(chrome(
                if area.width < 40 {
                    if area.width < 28 {
                        "A Auto  T Term  X Off"
                    } else {
                        "A Auto · T Terminal · X Off"
                    }
                } else {
                    "A Automatic · T Terminal only · X Off"
                },
                app.ascii,
            ))
            .style(muted),
            input,
        );
        frame.render_widget(
            Paragraph::new(chrome(
                if area.width < 65 {
                    if area.width < 28 {
                        "Tab details  ↑↓  Enter"
                    } else {
                        "Tab details · ↑↓ · Enter"
                    }
                } else {
                    "Tab details · ↑↓ read · Enter/Esc back"
                },
                app.ascii,
            ))
            .style(muted),
            hints,
        );
    } else {
        if composer {
            let title = if app.command_focus {
                " Question · Tab to type "
            } else if app.turns.is_empty() {
                " Question "
            } else {
                " Follow-up "
            };
            let mut block = Block::default()
                .borders(Borders::ALL)
                .title(chrome(title, app.ascii_input))
                .border_style(if app.command_focus { muted } else { amber });
            if app.ascii_input {
                block = block.border_set(ratatui::symbols::border::Set {
                    top_left: "+",
                    top_right: "+",
                    bottom_left: "+",
                    bottom_right: "+",
                    vertical_left: "|",
                    vertical_right: "|",
                    horizontal_top: "-",
                    horizontal_bottom: "-",
                });
            } else {
                block = block.border_type(ratatui::widgets::BorderType::Rounded);
            }
            frame.render_widget(block, composer_rect);
        }
        let shown = app
            .editor
            .text
            .replace('\n', if app.ascii_input { "~" } else { "↵" })
            .replace('\t', " ");
        let before = app.editor.text[..app.editor.cursor]
            .replace('\n', if app.ascii_input { "~" } else { "↵" })
            .replace('\t', " ");
        let cursor_cells = before.width();
        let prefix = if composer || area.width < 40 {
            "> "
        } else if app.ascii_input {
            "Ask > "
        } else {
            "Ask › "
        };
        let available = input.width.saturating_sub(prefix.width() as u16 + 1) as usize;
        let skip = cursor_cells.saturating_sub(available);
        let mut skipped = 0;
        let mut text = String::new();
        for g in shown.graphemes(true) {
            if skipped < skip {
                skipped += g.width();
            } else {
                text.push_str(g);
            }
        }
        let placeholder = text.is_empty();
        if placeholder {
            text = chrome(
                if app.turns.is_empty() {
                    "What would you like to do?"
                } else {
                    "Your follow-up …"
                },
                app.ascii_input,
            );
        }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix, amber),
                Span::styled(text, if placeholder { muted } else { base }),
            ])),
            input,
        );
        if !app.command_focus {
            frame.set_cursor_position((
                input.x
                    + prefix.width() as u16
                    + cursor_cells.saturating_sub(skipped).min(available) as u16,
                input.y,
            ));
        }
        let hint = if app.command_focus {
            if area.width < 65 {
                "↑↓ read · ←→ select · Enter use"
            } else {
                "↑↓ read · ←→ command · Enter use · Tab ask · Esc back"
            }
        } else if area.width < 65 {
            "Enter send · Tab read · Ctrl-Y"
        } else {
            "Enter send · Tab history · Ctrl-Y use · Esc close"
        };
        frame.render_widget(
            Paragraph::new(chrome(hint, app.ascii_input)).style(muted),
            hints,
        );
    }
}
