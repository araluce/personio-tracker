//! Rendering. Reads `App`, writes widgets — no state changes here.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::event::Severity;

use super::app::{App, Field, Mode, Pane, Status, edit_is_secret};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;
const WARN: Color = Color::Yellow;

pub fn render(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [log_area, side] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(body);

    let [config_area, summary_area] = Layout::vertical([
        Constraint::Length(Field::ALL.len() as u16 + 2),
        Constraint::Min(0),
    ])
    .areas(side);

    render_header(frame, app, header);
    render_log(frame, app, log_area);
    render_config(frame, app, config_area);
    render_summary(frame, app, summary_area);
    render_footer(frame, app, footer);

    if app.mode == Mode::Help {
        render_help(frame, body);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let (status_text, status_style) = match &app.status {
        Status::Idle => ("idle".to_string(), Style::new().fg(MUTED)),
        Status::Running => (
            "running".to_string(),
            Style::new().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Status::Finished => ("finished".to_string(), Style::new().fg(GOOD)),
        Status::Failed(error) => (format!("failed: {error}"), Style::new().fg(BAD)),
    };

    let mut spans = vec![
        Span::styled(
            " Personio Tracker ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("│ ", Style::new().fg(MUTED)),
        Span::styled(status_text, status_style),
    ];

    if app.has_unsaved_changes() {
        spans.push(Span::styled("  ● unsaved", Style::new().fg(WARN)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block("Live log", app.pane == Pane::Log);
    let inner_height = block.inner(area).height;

    let lines: Vec<Line> = app
        .log
        .iter()
        .map(|entry| {
            let style = match entry.severity {
                Severity::Good => Style::new().fg(GOOD),
                Severity::Error => Style::new().fg(BAD),
                Severity::Muted => Style::new().fg(MUTED),
                Severity::Info => Style::new(),
            };

            Line::from(vec![
                Span::styled(format!("{} ", entry.timestamp), Style::new().fg(MUTED)),
                Span::styled(entry.text.clone(), style),
            ])
        })
        .collect();

    // Following the tail means pinning the viewport to the bottom; otherwise
    // the manual offset wins, clamped so it cannot scroll past the end.
    let overflow = lines.len().saturating_sub(inner_height as usize) as u16;
    let scroll = if app.log_follow {
        overflow
    } else {
        app.log_scroll.min(overflow)
    };

    let body = if lines.is_empty() {
        vec![Line::styled(
            "Press t to start a tracking run.",
            Style::new().fg(MUTED),
        )]
    } else {
        lines
    };

    frame.render_widget(Paragraph::new(body).block(block).scroll((scroll, 0)), area);
}

fn render_config(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block("Configuration", app.pane == Pane::Config);

    let label_width = Field::ALL
        .iter()
        .map(|field| field.label().chars().count())
        .max()
        .unwrap_or(0);

    // Marker (2) + padded label + two spaces. Whatever is left belongs to the
    // value, which is ellipsised rather than hard-clipped by the buffer — a
    // long email in a narrow pane should still read as a truncated email.
    let value_width = (block.inner(area).width as usize).saturating_sub(label_width + 4);

    let lines: Vec<Line> = Field::ALL
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = app.pane == Pane::Config && index == app.field_index;
            let editing = selected && app.mode == Mode::Editing;

            let value = if editing {
                let shown = if edit_is_secret(*field) {
                    "•".repeat(app.edit_buffer.chars().count())
                } else {
                    app.edit_buffer.clone()
                };
                format!("{shown}_")
            } else {
                let value = app.field_display(*field);
                if value.is_empty() {
                    "—".to_string()
                } else {
                    value
                }
            };

            let value = truncate(&value, value_width);

            let marker = if selected { "▸ " } else { "  " };
            let value_style = if editing {
                Style::new().fg(WARN).add_modifier(Modifier::BOLD)
            } else if selected {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };

            Line::from(vec![
                Span::styled(marker, Style::new().fg(ACCENT)),
                Span::styled(
                    format!("{:<label_width$}  ", field.label()),
                    Style::new().fg(MUTED),
                ),
                Span::styled(value, value_style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block("Summary", false);

    let lines = match &app.summary {
        None => vec![Line::styled("No runs yet.", Style::new().fg(MUTED))],
        Some(summary) => vec![
            count_line("Tracked", summary.tracked_days.len(), GOOD),
            count_line("Skipped", summary.skipped_days.len(), MUTED),
            count_line(
                "Already registered",
                summary.already_registered_days.len(),
                MUTED,
            ),
            count_line("Errors", summary.errors.len(), BAD),
            count_line("Months visited", summary.months_visited, ACCENT),
            Line::raw(""),
            Line::styled(
                "Per-day detail is in the live log.",
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            ),
        ],
    };

    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// Shortens `value` to `width` columns, marking the cut with an ellipsis.
/// Counts characters rather than bytes so accents and the mask bullet survive.
fn truncate(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let length = value.chars().count();
    if length <= width {
        return value.to_string();
    }

    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn count_line(label: &str, value: usize, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<19}"), Style::new().fg(MUTED)),
        Span::styled(
            value.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(toast) = &app.toast {
        let style = if toast.is_error {
            Style::new().fg(BAD)
        } else {
            Style::new().fg(GOOD)
        };
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {}", toast.text), style)),
            area,
        );
        return;
    }

    let hints = match app.mode {
        Mode::Editing => " enter save · esc cancel · backspace delete",
        Mode::Help => " any key to close",
        Mode::Normal => {
            " t track · w save · tab pane · j/k move · enter edit · c clear · ? help · q quit"
        }
    };

    frame.render_widget(
        Paragraph::new(Line::styled(hints, Style::new().fg(MUTED))),
        area,
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::styled("Keys", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Line::raw(""),
        help_row("t", "start a tracking run"),
        help_row("w", "save settings to disk"),
        help_row("tab", "switch between log and configuration"),
        help_row("j / k", "move field, or scroll the log"),
        help_row("g / G", "jump to top / follow the log tail"),
        help_row("enter", "edit the selected field, or toggle it"),
        help_row("space", "toggle the selected flag"),
        help_row("c", "clear the log"),
        help_row("q", "quit (refused mid-run)"),
        help_row("Q / ctrl-c", "force quit, killing the browser"),
        Line::raw(""),
        Line::styled(
            "The password is written to the OS keychain and never read back.",
            Style::new().fg(MUTED),
        ),
    ];

    let popup = centered(area, 62, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(" Help ")
                .title_alignment(Alignment::Center),
        ),
        popup,
    );
}

fn help_row(keys: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {keys:<12}"), Style::new().fg(WARN)),
        Span::raw(description),
    ])
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    let border_style = if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(MUTED)
    };

    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(format!(" {title} "))
}

/// Clamps a popup to the available area so a small terminal cannot panic.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);

    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn draw(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_without_panicking_on_a_tiny_terminal() {
        let app = App::new(Settings::default(), false);
        draw(&app, 20, 6);
    }

    #[test]
    fn the_help_popup_fits_inside_a_small_body() {
        let mut app = App::new(Settings::default(), false);
        app.mode = Mode::Help;
        draw(&app, 30, 10);
    }

    #[test]
    fn centered_never_exceeds_its_container() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let popup = centered(area, 80, 40);

        assert_eq!(popup.width, 10);
        assert_eq!(popup.height, 4);
    }

    #[test]
    fn the_header_shows_the_unsaved_marker() {
        let mut app = App::new(Settings::default(), false);
        app.settings.employee_id = "42".into();

        assert!(draw(&app, 80, 24).contains("unsaved"));
    }

    #[test]
    fn a_stored_password_is_shown_as_a_mask_never_as_its_value() {
        let mut app = App::new(Settings::default(), false);
        app.keychain_has_password = true;

        let screen = draw(&app, 80, 24);
        assert!(screen.contains("•••••• saved"), "{screen}");
    }

    #[test]
    fn long_values_are_ellipsised_rather_than_clipped() {
        let mut app = App::new(Settings::default(), false);
        app.settings.personio_email = "someone@a-very-long-company-name.example".into();

        let screen = draw(&app, 80, 24);
        assert!(screen.contains('…'), "{screen}");
    }

    #[test]
    fn truncate_is_a_no_op_when_the_value_fits() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abc", 9), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn the_secret_field_masks_what_is_being_typed() {
        let mut app = App::new(Settings::default(), false);
        app.field_index = Field::ALL
            .iter()
            .position(|f| *f == Field::Password)
            .unwrap();
        app.mode = Mode::Editing;
        app.edit_buffer = "hunter2".into();

        let screen = draw(&app, 80, 24);
        assert!(!screen.contains("hunter2"), "{screen}");
        assert!(screen.contains("•••••••"), "{screen}");
    }
}
