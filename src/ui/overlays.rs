use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{AppState, InputMode, SignalModalState};
use crate::ui::Theme;

/// Renders any active overlay (`Filter` inline bar, `SignalModal`, or `Help` popup).
pub fn render(frame: &mut Frame, table_area: Rect, app: &AppState, theme: &Theme) {
    match &app.input_mode {
        InputMode::Normal => {}
        InputMode::Filter => render_filter_bar(frame, table_area, &app.filter_text, theme),
        InputMode::SignalModal(modal) => render_signal_modal(frame, frame.area(), modal, theme),
        InputMode::Help => render_help_modal(frame, frame.area(), theme),
    }
}

fn render_filter_bar(frame: &mut Frame, table_area: Rect, filter_text: &str, theme: &Theme) {
    if table_area.height == 0 || table_area.width == 0 {
        return;
    }

    let bar_height = if table_area.height >= 4 { 3 } else { 1 };
    let bar_area = Rect {
        x: table_area.x,
        y: table_area.bottom().saturating_sub(bar_height),
        width: table_area.width,
        height: bar_height,
    };

    frame.render_widget(Clear, bar_area);

    let line = Line::from(vec![
        Span::styled("Filter: ", theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled(filter_text, theme.text),
        Span::styled("_", theme.accent),
        Span::styled("   [Enter: apply | Esc: clear]", theme.muted),
    ]);

    let paragraph = if bar_height == 3 {
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.accent)
                .title(Span::styled(" Search / Filter ", theme.title)),
        )
    } else {
        Paragraph::new(line).style(theme.status_bar)
    };

    frame.render_widget(paragraph, bar_area);
}

fn render_signal_modal(frame: &mut Frame, area: Rect, modal: &SignalModalState, theme: &Theme) {
    let popup_area = centered_rect(54, 14, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent)
        .title(Span::styled(" Send Signal ", theme.title));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 4 || inner.width < 10 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Target: ", theme.muted),
            Span::styled(
                format!("PID {} ({})", modal.pid, modal.comm),
                theme.text.add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled("Select signal to send:", theme.muted)),
    ]);
    frame.render_widget(header, chunks[0]);

    let items: Vec<ListItem> = modal
        .options()
        .iter()
        .enumerate()
        .map(|(idx, opt)| {
            let selected = idx == modal.selected_signal_idx;
            let marker = if selected { "> " } else { "  " };
            let style = if selected {
                theme.selected_row.add_modifier(Modifier::BOLD)
            } else {
                theme.text
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{}", opt.label()),
                style,
            )))
        })
        .collect();

    frame.render_widget(List::new(items), chunks[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Enter/y", theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled(": Send  ", theme.muted),
        Span::styled("↑/↓/j/k", theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled(": Select  ", theme.muted),
        Span::styled("Esc/n", theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled(": Cancel", theme.muted),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(footer, chunks[2]);
}

fn render_help_modal(frame: &mut Frame, area: Rect, theme: &Theme) {
    let popup_area = centered_rect(66, 18, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent)
        .title(Span::styled(" Keybindings Help ", theme.title));

    let bindings = [
        ("↑ / ↓ / j / k", "Move selection up / down"),
        ("PgUp / PgDn", "Scroll page up / down"),
        ("Home / End / g / G", "Jump to top / bottom"),
        ("s / > / <", "Cycle sort column forward / backward"),
        ("r / I", "Reverse sort order"),
        ("t / F5", "Toggle process tree view"),
        ("Space / Enter", "Expand / collapse branch (in tree mode)"),
        ("/ / F3", "Filter processes by name, cmdline, user, PID"),
        ("x / K / F9", "Send POSIX signal to selected process"),
        ("? / F1", "Toggle this help window"),
        ("q / F10 / Ctrl+C", "Quit rstop"),
    ];

    let mut lines: Vec<Line> = bindings
        .into_iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<20}"),
                    theme.accent.add_modifier(Modifier::BOLD),
                ),
                Span::styled(desc, theme.text),
            ])
        })
        .collect();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Esc, Enter, ?, F1, or q to close",
        theme.muted,
    )));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup_area);
}

fn centered_rect(max_width: u16, max_height: u16, area: Rect) -> Rect {
    let width = max_width.min(area.width.saturating_sub(2)).max(1);
    let height = max_height.min(area.height.saturating_sub(2)).max(1);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
