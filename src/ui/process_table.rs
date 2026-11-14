use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use crate::app::{AppState, ProcessRow};
use crate::cli::SortColumn;
use crate::collectors::ProcessState;
use crate::ui::Theme;

/// Renders the sortable process table and synchronizes `TableState` with `app`.
pub fn render(frame: &mut Frame, area: Rect, app: &mut AppState, theme: &Theme) {
    let viewport_height = area.height.saturating_sub(3) as usize;
    app.adjust_scroll(viewport_height);

    let indicator = if app.sort_desc { "▼" } else { "▲" };
    let header_cells = [
        ("PID", Some(SortColumn::Pid)),
        ("USER", Some(SortColumn::User)),
        ("PRI", None),
        ("NI", None),
        ("VIRT", None),
        ("RES", None),
        ("S", None),
        ("CPU%", Some(SortColumn::Cpu)),
        ("MEM%", Some(SortColumn::Mem)),
        ("TIME+", Some(SortColumn::Time)),
        ("COMMAND", Some(SortColumn::Command)),
    ]
    .into_iter()
    .map(|(label, col)| {
        let is_sorted = col == Some(app.sort_col);
        let text = if is_sorted {
            format!("{label}{indicator}")
        } else {
            label.to_string()
        };
        let style = if is_sorted {
            theme.table_header_active
        } else {
            theme.table_header
        };
        Cell::from(Span::styled(text, style))
    });

    let header = Row::new(header_cells).height(1);

    let total_rows = app.visible_rows.len();
    let offset = app.scroll_offset.min(total_rows);
    let end = offset.saturating_add(viewport_height).min(total_rows);
    let visible_slice = &app.visible_rows[offset..end];

    let prefixes: Vec<String> = if app.tree_mode {
        build_tree_prefixes_range(&app.visible_rows, offset, end)
    } else {
        Vec::new()
    };

    let rows: Vec<Row> = visible_slice
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let p = &row.process;
            let cmd = if p.cmdline.is_empty() {
                p.comm.as_str()
            } else {
                p.cmdline.as_str()
            };
            let command_text = match prefixes.get(i) {
                Some(prefix) if !prefix.is_empty() => format!("{prefix}{cmd}"),
                _ => cmd.to_string(),
            };

            let state_style = match p.state {
                ProcessState::Running => theme.proc_running,
                ProcessState::Zombie => theme.proc_zombie,
                _ => theme.text,
            };

            Row::new(vec![
                Cell::from(format!("{:>7}", p.pid)),
                Cell::from(truncate_str(&p.user, 10)),
                Cell::from(format!("{:>3}", p.priority)),
                Cell::from(format!("{:>3}", p.nice)),
                Cell::from(format!("{:>7}", format_bytes(p.vsize_bytes))),
                Cell::from(format!("{:>7}", format_bytes(p.rss_bytes))),
                Cell::from(Span::styled(p.state.as_char().to_string(), state_style)),
                Cell::from(format!("{:>5.1}", p.cpu_pct)),
                Cell::from(format!("{:>5.1}", p.mem_pct)),
                Cell::from(format!("{:>9}", format_cpu_time(p.cpu_time_ticks))),
                Cell::from(command_text),
            ])
            .style(theme.text)
        })
        .collect();

    let widths = [
        Constraint::Length(7),  // PID
        Constraint::Length(10), // USER
        Constraint::Length(3),  // PRI
        Constraint::Length(3),  // NI
        Constraint::Length(7),  // VIRT
        Constraint::Length(7),  // RES
        Constraint::Length(1),  // S
        Constraint::Length(5),  // CPU%
        Constraint::Length(5),  // MEM%
        Constraint::Length(9),  // TIME+
        Constraint::Min(12),    // COMMAND
    ];

    let title = format!(" Processes ({total_rows}) ");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border)
        .title(Span::styled(title, theme.title));

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(theme.selected_row.add_modifier(Modifier::BOLD));

    let mut state = TableState::default();
    if offset < end {
        let clamped = app.selected_index.min(total_rows - 1);
        if clamped >= offset && clamped < end {
            state.select(Some(clamped - offset));
        }
    }

    frame.render_stateful_widget(table, area, &mut state);
}

fn build_tree_prefixes_range(rows: &[ProcessRow], start: usize, end: usize) -> Vec<String> {
    let mut prefixes = Vec::with_capacity(end.saturating_sub(start));
    let mut has_more_at_depth: Vec<bool> = Vec::new();

    for (idx, row) in rows[..end].iter().enumerate() {
        if row.tree_depth > 0 {
            if has_more_at_depth.len() < row.tree_depth {
                has_more_at_depth.resize(row.tree_depth, false);
            }
            has_more_at_depth[row.tree_depth - 1] = !row.is_last_child;
        }

        if idx < start {
            continue;
        }

        let mut prefix = String::new();
        if row.tree_depth > 0 {
            for &has_more in &has_more_at_depth[..row.tree_depth - 1] {
                if has_more {
                    prefix.push_str("│ ");
                } else {
                    prefix.push_str("  ");
                }
            }
            if row.is_last_child {
                prefix.push_str("└─");
            } else {
                prefix.push_str("├─");
            }
        }
        if row.has_children {
            if row.is_collapsed {
                prefix.push_str("[+] ");
            } else {
                prefix.push_str("[-] ");
            }
        }
        prefixes.push(prefix);
    }

    prefixes
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else if max_chars > 1 {
        let mut out: String = s.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Formats a byte count into compact human-readable units (B, K, M, G, T).
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;

    let b = bytes as f64;
    if b >= TIB {
        format!("{:.1}T", b / TIB)
    } else if b >= GIB {
        format!("{:.1}G", b / GIB)
    } else if b >= MIB {
        format!("{:.1}M", b / MIB)
    } else if b >= KIB {
        format!("{:.0}K", b / KIB)
    } else {
        format!("{bytes}B")
    }
}

/// Formats CPU time ticks (100 ticks/sec) as `MM:SS.cc` or `HH:MM:SS`.
pub fn format_cpu_time(ticks: u64) -> String {
    let total_centis = ticks;
    let total_secs = total_centis / 100;
    let centis = total_centis % 100;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}.{centis:02}")
    }
}
