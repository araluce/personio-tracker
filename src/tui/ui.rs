//! Rendering. Reads `App`, writes widgets — no state changes here.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use chrono::{Datelike, Local};

use crate::calendar::{DayState, MonthKey};
use crate::event::Severity;

use super::app::{App, Field, Mode, Pane, Status, edit_is_secret};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;
const WARN: Color = Color::Yellow;

/// Columns kept clear either side of a field hint.
const HINT_PADDING: usize = 1;

/// Braille spinner, one glyph per tick.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Weekday initials, aligned with the two-column cells below them.
const WEEKDAYS: &str = "Mo Tu We Th Fr Sa Su";

/// A resolved day. Seven eighths of a block rather than a whole one: the
/// missing eighth is the gap that keeps a week's cells off the week above them.
const CELL: &str = "▇▇";

/// A day nothing is known about. Distinct from `CELL` in symbol as well as in
/// colour, so the grid still reads where the palette is unusual.
const EMPTY_CELL: &str = "··";

/// The travelling highlight, head first. Ending on the border's own colour is
/// what makes it read as a trail fading out rather than a row of dashes.
const TRAIL: [Color; 5] = [Color::White, ACCENT, ACCENT, MUTED, MUTED];

pub fn render(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [log_area, side] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(body);

    // The hint belongs to the selected row, so it is only reserved while the
    // configuration pane owns the selection marker, and only as tall as the
    // text actually needs.
    let hint = if app.pane == Pane::Config {
        app.selected_field().hint()
    } else {
        None
    };
    let hint_width = (side.width as usize).saturating_sub(HINT_PADDING * 2);
    let hint_height = hint
        .map(|text| wrapped_height(text, hint_width))
        .filter(|height| *height > 0)
        .map_or(0, |height| height + 1);

    let month = app.viewed_month();

    let [config_area, hint_area, calendar_area, summary_area] = Layout::vertical([
        Constraint::Length(Field::ALL.len() as u16 + 2),
        Constraint::Length(hint_height),
        Constraint::Length(calendar_height(month)),
        Constraint::Min(0),
    ])
    .areas(side);

    render_header(frame, app, header);
    render_log(frame, app, log_area);
    if app.is_running() {
        render_activity_trail(frame, log_area, app.tick);
    }
    render_config(frame, app, config_area);
    if let Some(text) = hint {
        render_hint(frame, text, hint_area);
    }
    render_calendar(frame, app, month, calendar_area);
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
            format!("{} running", spinner_frame(app.tick)),
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
                Severity::Warning => Style::new().fg(WARN),
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

fn spinner_frame(tick: u64) -> &'static str {
    SPINNER[(tick % SPINNER.len() as u64) as usize]
}

/// A highlight travelling along the foot of the log pane while a run is in
/// flight. Personio spends whole seconds on a single click, so the log can sit
/// still for a while — this is what says "working", not "hung".
///
/// It runs along the bottom border because the top one carries the pane title,
/// and a trail crossing it would blank out the letters underneath. Drawn
/// straight into the buffer over the border the block already painted, so it
/// stays out of the layout: nothing shifts when a run starts.
fn render_activity_trail(frame: &mut Frame, area: Rect, tick: u64) {
    // The corners belong to the block, so the trail runs between them.
    let track = area.width.saturating_sub(2);
    if track == 0 || area.height == 0 {
        return;
    }

    let head = (tick % u64::from(track)) as u16;
    let foot = area.y + area.height - 1;

    // Tail first: where a narrow pane folds the trail onto itself, the head
    // must be the cell that survives.
    for (offset, colour) in TRAIL.iter().enumerate().rev() {
        let offset = offset as u16;
        if offset >= track {
            continue;
        }

        let column =
            ((u32::from(head) + u32::from(track) - u32::from(offset)) % u32::from(track)) as u16;
        let style = if offset == 0 {
            Style::new().fg(*colour).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(*colour)
        };

        if let Some(cell) = frame
            .buffer_mut()
            .cell_mut(Position::new(area.x + 1 + column, foot))
        {
            cell.set_symbol("━").set_style(style);
        }
    }
}

/// One-line explanation of where the selected field's value comes from. The
/// leading blank line separates it from the pane border above.
fn render_hint(frame: &mut Frame, text: &str, area: Rect) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                text.to_string(),
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            ),
        ])
        .block(Block::new().padding(Padding::horizontal(HINT_PADDING as u16)))
        .wrap(Wrap { trim: true }),
        area,
    );
}

/// Rows the month's grid needs: its own borders, the weekday header, and one
/// row per week the month spills into.
fn calendar_height(month: MonthKey) -> u16 {
    let weeks = (leading_blanks(month) + month.length() as usize).div_ceil(7);
    weeks as u16 + 3
}

/// How many cells the 1st is pushed along so it lands under its weekday.
fn leading_blanks(month: MonthKey) -> usize {
    month
        .day(1)
        .map_or(0, |date| date.weekday().num_days_from_monday() as usize)
}

/// The month at a glance: one cell per day, coloured by what became of it.
///
/// Drawn from the record on disk, so it says what has been tracked before the
/// app has run anything — which is the point. A run repaints it live as its
/// days resolve.
fn render_calendar(frame: &mut Frame, app: &App, month: MonthKey, area: Rect) {
    let today = Local::now().date_naive();
    let mut lines = vec![Line::styled(WEEKDAYS, Style::new().fg(MUTED))];
    let mut week: Vec<Span> = vec![Span::raw("  "); leading_blanks(month)];

    for day in 1..=month.length() {
        let (symbol, colour) = cell(app.calendar().state(month, day));
        let mut style = Style::new().fg(colour);
        if month.day(day) == Some(today) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }

        week.push(Span::styled(symbol, style));
        if week.len() == 7 {
            lines.push(week_line(std::mem::take(&mut week)));
        }
    }
    if !week.is_empty() {
        lines.push(week_line(week));
    }

    // Says so rather than leaving a first-time user to wonder whether a grid
    // of empty cells means "nothing tracked" or "nothing working".
    let title = if app.calendar().has_month(month) {
        month.label()
    } else {
        format!("{} · no record", month.label())
    };

    // Centred, because a graphic flush against the border reads as text that
    // ran out of room.
    let margin = area
        .width
        .saturating_sub(2)
        .saturating_sub(WEEKDAYS.len() as u16)
        / 2;

    frame.render_widget(
        Paragraph::new(lines).block(pane_block(&title, false).padding(Padding::left(margin))),
        area,
    );
}

/// Joins a week's cells with the single space the header is spaced by.
fn week_line(cells: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = Vec::with_capacity(cells.len() * 2);
    for (index, cell) in cells.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(cell);
    }
    Line::from(spans)
}

/// Both greens mean "there are hours on the day"; the brighter one is ours.
fn cell(state: Option<DayState>) -> (&'static str, Color) {
    match state {
        Some(DayState::Tracked) => (CELL, Color::LightGreen),
        Some(DayState::AlreadyRegistered) => (CELL, Color::Green),
        Some(DayState::OffDay) => (CELL, Color::LightBlue),
        Some(DayState::Holiday) => (CELL, Color::Magenta),
        Some(DayState::Weekend) => (CELL, MUTED),
        None => (EMPTY_CELL, MUTED),
    }
}

/// Lines `text` occupies once wrapped at `width` columns, so the hint area is
/// reserved exactly and a hint never clips itself.
fn wrapped_height(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 0;
    }

    let mut lines = 1u16;
    let mut column = 0usize;

    for word in text.split_whitespace() {
        let length = word.chars().count();

        if column > 0 && column + 1 + length > width {
            lines += 1;
            column = 0;
        }

        if length > width {
            // A word wider than the pane is broken across lines mid-word.
            lines += ((length - 1) / width) as u16;
            let remainder = length % width;
            column = if remainder == 0 { width } else { remainder };
        } else {
            column = if column == 0 {
                length
            } else {
                column + 1 + length
            };
        }
    }

    lines
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
        // Trimmed to fit 80 columns; `?` carries the full list, including the
        // two that came out of here to make room.
        Mode::Normal => " t track · w save · j/k move · h/l month · enter edit · ? help · q quit",
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
        help_row("h / l", "page the month grid back and forward"),
        help_row("enter", "edit the selected field, or toggle it"),
        help_row("space", "toggle the selected flag"),
        help_row("c", "clear the log"),
        help_row("q", "quit (refused mid-run)"),
        help_row("Q / ctrl-c", "force quit, killing the browser"),
        Line::raw(""),
        Line::styled(
            "Month grid",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        legend_line(&[
            (Color::LightGreen, "tracked by a run"),
            (Color::Green, "already registered"),
        ]),
        legend_line(&[
            (Color::LightBlue, "absence or day off"),
            (Color::Magenta, "public holiday"),
        ]),
        legend_line(&[(MUTED, "weekend, or nothing known about the day yet")]),
        Line::raw(""),
        Line::styled(
            "The password is written to the OS keychain and never read back.",
            Style::new().fg(MUTED),
        ),
    ];

    // Sized from its own content, so a line added here cannot silently lose
    // its tail: the note below has been one character too wide all along.
    let width = lines
        .iter()
        .map(|line| line.width() as u16)
        .max()
        .unwrap_or(0)
        + 2;
    let popup = centered(area, width, lines.len() as u16 + 2);
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

/// Swatches inline with their meaning, several to a line: the popup has to
/// stay short enough for a 24-row terminal.
fn legend_line(entries: &[(Color, &'static str)]) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    for (colour, meaning) in entries {
        spans.push(Span::styled(CELL, Style::new().fg(*colour)));
        spans.push(Span::styled(
            format!(" {meaning}   "),
            Style::new().fg(MUTED),
        ));
    }
    Line::from(spans)
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
    use crate::calendar::{Calendar, DayState};
    use crate::config::Settings;
    use crate::event::TrackingEvent;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

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
        let app = App::new(Settings::default(), false, Calendar::default());
        draw(&app, 20, 6);
    }

    /// Both dimensions: the legend made the popup taller, and the note at the
    /// bottom is wider than the width it used to be given.
    #[test]
    fn the_help_popup_shows_all_of_itself_on_a_standard_terminal() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        app.mode = Mode::Help;
        let screen = draw(&app, 80, 24);

        assert!(screen.contains("force quit"), "{screen}");
        assert!(
            screen.contains("nothing known about the day yet"),
            "{screen}"
        );
        assert!(screen.contains("never read back."), "{screen}");
    }

    #[test]
    fn the_help_popup_fits_inside_a_small_body() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
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
        let mut app = App::new(Settings::default(), false, Calendar::default());
        app.settings.employee_id = "42".into();

        assert!(draw(&app, 80, 24).contains("unsaved"));
    }

    #[test]
    fn a_stored_password_is_shown_as_a_mask_never_as_its_value() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        app.keychain_has_password = true;

        let screen = draw(&app, 80, 24);
        assert!(screen.contains("•••••• saved"), "{screen}");
    }

    #[test]
    fn long_values_are_ellipsised_rather_than_clipped() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
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

    fn select(app: &mut App, field: Field) {
        app.field_index = Field::ALL.iter().position(|f| *f == field).unwrap();
    }

    #[test]
    fn the_selected_field_explains_where_its_value_comes_from() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        select(&mut app, Field::EmployeeId);

        let screen = draw(&app, 80, 24);
        assert!(screen.contains("employee/1234"), "{screen}");
    }

    #[test]
    fn a_field_with_no_hint_reserves_no_room_for_one() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        select(&mut app, Field::Email);

        let with_hint = {
            let mut app = App::new(Settings::default(), false, Calendar::default());
            select(&mut app, Field::EmployeeId);
            draw(&app, 80, 24)
        };

        let without_hint = draw(&app, 80, 24);
        assert!(!without_hint.contains("employee/1234"));
        assert_ne!(with_hint, without_hint);
    }

    #[test]
    fn the_log_pane_owning_the_selection_hides_the_hint() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        select(&mut app, Field::EmployeeId);
        app.pane = Pane::Log;

        assert!(!draw(&app, 80, 24).contains("employee/1234"));
    }

    #[test]
    fn a_hint_never_panics_on_a_tiny_terminal() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        select(&mut app, Field::EmployeeId);

        draw(&app, 20, 6);
        draw(&app, 4, 30);
    }

    #[test]
    fn wrapped_height_counts_the_lines_a_hint_needs() {
        assert_eq!(wrapped_height("short", 20), 1);
        assert_eq!(wrapped_height("one two three", 7), 2);
        assert_eq!(wrapped_height("a b", 1), 2);
        assert_eq!(wrapped_height("anything", 0), 0);
    }

    #[test]
    fn wrapped_height_accounts_for_words_wider_than_the_pane() {
        assert_eq!(wrapped_height("abcdefghij", 5), 2);
        assert_eq!(wrapped_height("abcdefghijk", 5), 3);
        assert_eq!(wrapped_height("ab abcdefghij", 5), 3);
    }

    #[test]
    fn every_hint_fits_the_pane_it_is_measured_against() {
        for field in Field::ALL {
            let Some(text) = field.hint() else { continue };
            let mut app = App::new(Settings::default(), false, Calendar::default());
            select(&mut app, field);

            let screen = draw(&app, 80, 24);
            let last_word = text.split_whitespace().last().unwrap();
            assert!(
                screen.contains(last_word),
                "{field:?} hint clipped: {screen}"
            );
        }
    }

    fn running(tick: u64) -> App {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        app.status = Status::Running;
        app.tick = tick;
        app
    }

    #[test]
    fn dump_final() {
        let month = MonthKey::current();
        let mut calendar = Calendar::default();
        for day in 1..=month.length() {
            let weekday = month.day(day).unwrap().weekday().num_days_from_monday();
            let state = if weekday >= 5 {
                DayState::Weekend
            } else if day == 1 {
                DayState::Holiday
            } else if (14..=18).contains(&day) {
                DayState::OffDay
            } else if day % 4 == 0 {
                DayState::AlreadyRegistered
            } else {
                DayState::Tracked
            };
            calendar.set(month, day, state);
        }
        let mut app = App::new(Settings::default(), true, calendar);
        app.settings.personio_email = "me@acme.com".into();
        app.settings.personio_company = "acme".into();
        app.settings.employee_id = "1234".into();
        app.mark_settings_saved();
        for line in [
            "Restored saved session",
            "Valid session",
            "Opening Time Tracking",
            "Tracking month (30 rows)",
            "4: shift registered",
            "Reached today's row at index 6",
        ] {
            app.push_log(line.into(), Severity::Info);
        }
        println!("{}", draw(&app, 80, 24));
        println!("--- help ---");
        app.mode = Mode::Help;
        println!("{}", draw(&app, 80, 30));
    }

    /// Column of `needle` in a rendered row, counted in cells rather than
    /// bytes: the pane borders are multi-byte, so a byte offset is not a
    /// column, and reading the buffer at one lands on the wrong weekday.
    fn column_of(row: &str, needle: &str) -> Option<usize> {
        let byte_offset = row.find(needle)?;
        Some(row[..byte_offset].chars().count())
    }

    /// The colour of every grid cell, which is the part `draw` cannot show:
    /// the reported bug was entirely about which column a colour landed in.
    fn draw_styled(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The bug as it was reported: Friday and Saturday shown as the weekend.
    /// Asserted on colour by weekday column, which is the only form that
    /// catches a grid shifted by one day.
    #[test]
    fn the_weekend_is_grey_under_saturday_and_sunday() {
        let month = MonthKey::current();
        let mut calendar = Calendar::default();
        for day in 1..=month.length() {
            let weekday = month.day(day).unwrap().weekday();
            let state = if weekday.number_from_monday() > 5 {
                DayState::Weekend
            } else {
                DayState::Tracked
            };
            calendar.set(month, day, state);
        }

        let app = App::new(Settings::default(), false, calendar);
        let screen = draw(&app, 80, 40);
        let rows: Vec<&str> = screen.lines().collect();
        let header = rows
            .iter()
            .position(|row| row.contains(WEEKDAYS))
            .expect("the weekday header is drawn");
        let left = column_of(rows[header], WEEKDAYS).unwrap();
        let buffer = draw_styled(&app, 80, 40);

        // A buffer cell holds one grapheme, so a two-column cell is two of
        // them; only the first of each is read.
        let glyph = CELL.chars().next().unwrap().to_string();

        let mut checked = 0;
        for week in 1..=6 {
            for column in 0..7usize {
                // Three columns per cell: two for the cell, one for the gap.
                let cell = &buffer[((left + column * 3) as u16, (header + week) as u16)];
                if cell.symbol() != glyph {
                    continue;
                }

                let expected = if column >= 5 {
                    MUTED
                } else {
                    Color::LightGreen
                };
                assert_eq!(
                    cell.fg, expected,
                    "week {week}, column {column} of {WEEKDAYS}"
                );
                checked += 1;
            }
        }

        assert_eq!(checked, month.length() as usize, "every day is coloured");
    }

    fn month_of(year: i32, month: u32) -> MonthKey {
        MonthKey { year, month }
    }

    fn filled_month() -> (MonthKey, Calendar) {
        let month = MonthKey::current();
        let mut calendar = Calendar::default();
        for day in 1..=month.length() {
            calendar.set(month, day, DayState::Tracked);
        }
        (month, calendar)
    }

    #[test]
    fn the_grid_draws_one_cell_per_day_of_the_month() {
        let (month, calendar) = filled_month();
        let screen = draw(&App::new(Settings::default(), false, calendar), 80, 40);

        assert_eq!(screen.matches(CELL).count(), month.length() as usize);
    }

    /// Week rows are adjacent lines, so a cell as tall as its line box touches
    /// the one above it and a fully tracked month renders as seven solid bars
    /// instead of a calendar. Nothing can be put between the rows — a blank
    /// line would cost the grid its fifth week — so the gap has to come out of
    /// the glyph: a lower block, shorter than the line it sits on.
    ///
    /// The full block caused this, but excluding it alone is not enough: every
    /// left block from `▉` up is full height too, and any of them would bring
    /// the bars straight back.
    #[test]
    fn a_grid_cell_is_shorter_than_the_line_it_sits_on() {
        for glyph in CELL.chars() {
            assert!(
                ('▁'..='▇').contains(&glyph),
                "{glyph} is not a lower partial block, so weeks touch: {CELL}"
            );
        }
    }

    #[test]
    fn a_month_with_no_record_is_all_empty_cells() {
        let month = MonthKey::current();
        let screen = draw(
            &App::new(Settings::default(), false, Calendar::default()),
            80,
            40,
        );

        assert_eq!(screen.matches(EMPTY_CELL).count(), month.length() as usize);
        assert!(screen.contains("no record"), "{screen}");
    }

    #[test]
    fn a_month_with_a_record_drops_the_no_record_note() {
        let (_, calendar) = filled_month();
        let screen = draw(&App::new(Settings::default(), false, calendar), 80, 40);

        assert!(!screen.contains("no record"), "{screen}");
    }

    /// The 1st has to sit under its own weekday, or every colour in the grid
    /// is telling the truth about the wrong day.
    #[test]
    fn the_first_of_the_month_lines_up_under_its_weekday() {
        let (month, calendar) = filled_month();
        let screen = draw(&App::new(Settings::default(), false, calendar), 80, 40);
        let rows: Vec<&str> = screen.lines().collect();

        let header = rows
            .iter()
            .position(|row| row.contains(WEEKDAYS))
            .expect("the weekday header is drawn");
        let header_column = column_of(rows[header], WEEKDAYS).unwrap();

        // Derived from the calendar rather than from `leading_blanks`, which
        // is the thing under test: an expectation built from it would agree
        // with any answer it gave.
        let weekday = month.day(1).unwrap().weekday().num_days_from_monday() as usize;

        // Three columns per cell: two for the cell, one for the gap.
        let expected = header_column + weekday * 3;
        assert_eq!(
            column_of(rows[header + 1], CELL),
            Some(expected),
            "{screen}"
        );
    }

    #[test]
    fn leading_blanks_follow_the_weekday_the_month_opens_on() {
        // 1 September 2026 is a Tuesday, 1 November 2026 a Sunday.
        assert_eq!(leading_blanks(month_of(2026, 9)), 1);
        assert_eq!(leading_blanks(month_of(2026, 11)), 6);
    }

    #[test]
    fn a_month_that_spills_into_a_sixth_week_gets_a_sixth_row() {
        // Borders and header, plus five weeks for September 2026 (30 days
        // from a Tuesday) and six for November (30 from a Sunday).
        assert_eq!(calendar_height(month_of(2026, 9)), 8);
        assert_eq!(calendar_height(month_of(2026, 11)), 9);
        assert_eq!(calendar_height(month_of(2026, 2)), 8);
    }

    #[test]
    fn the_grid_never_panics_where_it_does_not_fit() {
        let (_, calendar) = filled_month();
        for (width, height) in [(80, 14), (20, 6), (4, 4), (1, 1)] {
            draw(
                &App::new(Settings::default(), false, calendar.clone()),
                width,
                height,
            );
        }
    }

    /// A check the run could not make is not a failure, and must not be
    /// dressed as one.
    #[test]
    fn a_warning_in_the_log_is_not_coloured_like_an_error() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
        app.apply_tracking_event(TrackingEvent::HoursPendingCheckMissing(
            "No tracked month hours found (count=0)".into(),
        ));

        let screen = draw(&app, 80, 40);
        let rows: Vec<&str> = screen.lines().collect();
        let row = rows
            .iter()
            .position(|row| row.contains("No tracked month hours"))
            .expect("the line is logged");
        let column = column_of(rows[row], "No tracked").unwrap();

        let buffer = draw_styled(&app, 80, 40);
        assert_eq!(buffer[(column as u16, row as u16)].fg, WARN);
    }

    #[test]
    fn the_status_carries_a_spinner_while_running() {
        let first = draw(&running(0), 80, 24);
        let later = draw(&running(4), 80, 24);

        assert!(first.contains(SPINNER[0]), "{first}");
        assert!(later.contains(SPINNER[4]), "{later}");
        assert!(
            !draw(
                &App::new(Settings::default(), false, Calendar::default()),
                80,
                24
            )
            .contains(SPINNER[0])
        );
    }

    #[test]
    fn spinner_frames_cycle_rather_than_run_out() {
        assert_eq!(spinner_frame(0), spinner_frame(SPINNER.len() as u64));
        assert_eq!(spinner_frame(u64::MAX), SPINNER[(u64::MAX % 10) as usize]);
    }

    #[test]
    fn the_activity_trail_shows_only_while_running() {
        assert!(draw(&running(0), 80, 24).contains('━'));
        assert!(
            !draw(
                &App::new(Settings::default(), false, Calendar::default()),
                80,
                24
            )
            .contains('━')
        );
    }

    #[test]
    fn the_trail_runs_along_the_foot_of_the_log_pane() {
        let screen = draw(&running(0), 80, 24);
        let rows: Vec<&str> = screen.lines().collect();

        // The footer is the last row; the log pane closes on the one above it.
        let foot = rows[rows.len() - 2];
        assert!(foot.contains('━'), "{screen}");
        assert!(
            rows[..rows.len() - 2].iter().all(|row| !row.contains('━')),
            "{screen}"
        );
    }

    /// The trail used to run along the top border, where it blanked out the
    /// letters of the pane title as it passed.
    #[test]
    fn the_pane_title_is_never_overwritten_by_the_trail() {
        for tick in 0..80 {
            let screen = draw(&running(tick), 80, 24);
            assert!(screen.contains("Live log"), "tick {tick}: {screen}");
        }
    }

    #[test]
    fn the_trail_never_panics_on_a_pane_too_narrow_for_it() {
        for (width, height) in [(20, 6), (4, 4), (2, 2), (1, 1)] {
            draw(&running(3), width, height);
        }
    }

    #[test]
    fn the_secret_field_masks_what_is_being_typed() {
        let mut app = App::new(Settings::default(), false, Calendar::default());
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
