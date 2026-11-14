use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Sparkline};

use crate::app::AppState;
use crate::collectors::SystemSnapshot;
use crate::ui::Theme;

/// Computes an adaptive header height based on core count and terminal dimensions.
pub fn desired_height(app: &AppState, area_width: u16, area_height: u16) -> u16 {
    let num_cores = app
        .snapshot
        .as_ref()
        .map(|s| s.cpu.cores.len())
        .unwrap_or(0);

    let left_width = area_width / 2;
    let cols = core_columns(left_width);
    let core_rows = if num_cores == 0 {
        1
    } else {
        num_cores.div_ceil(cols) as u16
    };

    // Left: 1 (total gauge) + 2 (sparkline) + core_rows + 2 (borders)
    // Right: 1 (mem) + 1 (swap) + 1 (load/uptime) + 1 (tasks/net) + 2 (borders) = 6
    let ideal = (5 + core_rows).max(7);
    let max_allowed = area_height.saturating_sub(9).max(6);
    ideal.min(max_allowed)
}

/// Renders the system overview header (CPU gauges/sparkline, per-core meters, memory/swap, load/net).
pub fn render(frame: &mut Frame, area: Rect, app: &AppState, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    match &app.snapshot {
        Some(snapshot) => {
            render_cpu_panel(frame, chunks[0], snapshot, theme);
            render_sys_panel(frame, chunks[1], snapshot, theme);
        }
        None => {
            let cpu_block = Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border)
                .title(" CPU ");
            frame.render_widget(
                Paragraph::new("Collecting CPU metrics...")
                    .style(theme.muted)
                    .block(cpu_block),
                chunks[0],
            );

            let sys_block = Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border)
                .title(" System ");
            frame.render_widget(
                Paragraph::new("Collecting system metrics...")
                    .style(theme.muted)
                    .block(sys_block),
                chunks[1],
            );
        }
    }
}

fn core_columns(panel_width: u16) -> usize {
    if panel_width >= 64 {
        4
    } else if panel_width >= 34 {
        2
    } else {
        1
    }
}

fn render_cpu_panel(frame: &mut Frame, area: Rect, snapshot: &SystemSnapshot, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border)
        .title(Span::styled(" CPU ", theme.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let spark_height = if inner.height >= 4 { 2 } else { 0 };
    let cores_height = inner.height.saturating_sub(1 + spark_height);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(spark_height),
            Constraint::Length(cores_height),
        ])
        .split(inner);

    let total_pct = snapshot.cpu.total.total_pct.clamp(0.0, 100.0);
    let total_label = format!(
        "Total {:5.1}% (usr {:4.1}% sys {:4.1}% io {:4.1}%)",
        total_pct,
        snapshot.cpu.total.user_pct.clamp(0.0, 100.0),
        snapshot.cpu.total.sys_pct.clamp(0.0, 100.0),
        snapshot.cpu.total.iowait_pct.clamp(0.0, 100.0),
    );
    let total_gauge = Gauge::default()
        .gauge_style(theme.usage_style(total_pct))
        .ratio(total_pct / 100.0)
        .label(Span::styled(
            total_label,
            theme.text.add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(total_gauge, rows[0]);

    if spark_height > 0 {
        let history_data: Vec<u64> = snapshot
            .cpu
            .history
            .iter()
            .map(|v| v.clamp(0.0, 100.0).round() as u64)
            .collect();
        let sparkline = Sparkline::default()
            .data(&history_data)
            .max(100)
            .style(theme.accent);
        frame.render_widget(sparkline, rows[1]);
    }

    if cores_height > 0 && !snapshot.cpu.cores.is_empty() {
        render_core_meters(frame, rows[2], snapshot, theme);
    }
}

fn render_core_meters(frame: &mut Frame, area: Rect, snapshot: &SystemSnapshot, theme: &Theme) {
    let cols = core_columns(area.width);
    let max_rows = area.height as usize;
    let cores = &snapshot.cpu.cores;
    let mut lines = Vec::with_capacity(max_rows);

    let col_width = (area.width as usize / cols).max(12);
    // Reserve space for "C00 [" + "] 100.0%" (~14 chars)
    let bar_width = col_width.saturating_sub(14).clamp(3, 16);

    for row_idx in 0..max_rows {
        let start = row_idx * cols;
        if start >= cores.len() {
            break;
        }
        let end = (start + cols).min(cores.len());
        let mut spans = Vec::new();
        for (offset, core) in cores[start..end].iter().enumerate() {
            let core_id = start + offset;
            let pct = core.total_pct.clamp(0.0, 100.0);
            let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let empty = bar_width - filled;

            if offset > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(format!("C{core_id:<2}["), theme.muted));
            spans.push(Span::styled("|".repeat(filled), theme.usage_style(pct)));
            spans.push(Span::styled(" ".repeat(empty), theme.muted));
            spans.push(Span::styled(format!("]{pct:5.1}%"), theme.text));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_sys_panel(frame: &mut Frame, area: Rect, snapshot: &SystemSnapshot, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border)
        .title(Span::styled(" System ", theme.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    // Memory gauge
    let mem = &snapshot.mem;
    let mem_ratio = if mem.total_kb > 0 {
        (mem.used_kb as f64 / mem.total_kb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mem_pct = mem_ratio * 100.0;
    let mem_label = if inner.width >= 44 {
        format!(
            "Mem {}/{} (buf {}, cache {})",
            format_kib(mem.used_kb),
            format_kib(mem.total_kb),
            format_kib(mem.buffers_kb),
            format_kib(mem.cached_kb),
        )
    } else {
        format!(
            "Mem {}/{} (c {})",
            format_kib(mem.used_kb),
            format_kib(mem.total_kb),
            format_kib(mem.cached_kb),
        )
    };
    let mem_gauge = Gauge::default()
        .gauge_style(theme.usage_style(mem_pct))
        .ratio(mem_ratio)
        .label(Span::styled(mem_label, theme.text));
    frame.render_widget(mem_gauge, chunks[0]);

    // Swap gauge
    let swap_ratio = if mem.swap_total_kb > 0 {
        (mem.swap_used_kb as f64 / mem.swap_total_kb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let swap_pct = swap_ratio * 100.0;
    let swap_label = format!(
        "Swp {}/{}",
        format_kib(mem.swap_used_kb),
        format_kib(mem.swap_total_kb),
    );
    let swap_gauge = Gauge::default()
        .gauge_style(theme.usage_style(swap_pct))
        .ratio(swap_ratio)
        .label(Span::styled(swap_label, theme.text));
    frame.render_widget(swap_gauge, chunks[1]);

    // Load, Uptime, Tasks, Net rates
    let load = &snapshot.load;
    let (rx_rate, tx_rate) = aggregate_net_rates(snapshot);
    let info_lines = vec![
        Line::from(vec![
            Span::styled("Load: ", theme.muted),
            Span::styled(
                format!("{:.2} {:.2} {:.2}", load.load1, load.load5, load.load15),
                theme.text,
            ),
            Span::styled("  Up: ", theme.muted),
            Span::styled(format_uptime(load.uptime_secs), theme.text),
        ]),
        Line::from(vec![
            Span::styled("Tasks: ", theme.muted),
            Span::styled(
                format!(
                    "{} proc, {}/{} thr",
                    snapshot.processes.len(),
                    load.running_threads,
                    load.total_threads
                ),
                theme.text,
            ),
        ]),
        Line::from(vec![
            Span::styled("Net: ", theme.muted),
            Span::styled(format!("Rx {}", format_rate(rx_rate)), theme.accent),
            Span::raw("  "),
            Span::styled(format!("Tx {}", format_rate(tx_rate)), theme.accent),
        ]),
    ];

    frame.render_widget(Paragraph::new(info_lines), chunks[2]);
}

fn aggregate_net_rates(snapshot: &SystemSnapshot) -> (f64, f64) {
    let non_lo: Vec<_> = snapshot
        .net
        .iter()
        .filter(|iface| iface.name != "lo")
        .collect();
    let list = if non_lo.is_empty() {
        snapshot.net.iter().collect::<Vec<_>>()
    } else {
        non_lo
    };

    list.into_iter().fold((0.0, 0.0), |(rx, tx), iface| {
        (rx + iface.rx_bytes_per_sec, tx + iface.tx_bytes_per_sec)
    })
}

/// Formats a KiB value into human-readable KiB, MiB, or GiB.
pub fn format_kib(kib: u64) -> String {
    const MIB: f64 = 1024.0;
    const GIB: f64 = 1024.0 * 1024.0;

    let val = kib as f64;
    if val >= GIB {
        format!("{:.1}GiB", val / GIB)
    } else if val >= MIB {
        format!("{:.0}MiB", val / MIB)
    } else {
        format!("{kib}KiB")
    }
}

/// Formats a byte-per-second rate into human-readable B/s, KiB/s, MiB/s, or GiB/s.
pub fn format_rate(bytes_per_sec: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let rate = bytes_per_sec.max(0.0);
    if rate >= GIB {
        format!("{:.1}GiB/s", rate / GIB)
    } else if rate >= MIB {
        format!("{:.1}MiB/s", rate / MIB)
    } else if rate >= KIB {
        format!("{:.1}KiB/s", rate / KIB)
    } else {
        format!("{rate:.0}B/s")
    }
}

/// Formats uptime seconds into `Xd HH:MM:SS` or `HH:MM:SS`.
pub fn format_uptime(uptime_secs: f64) -> String {
    let total = uptime_secs.max(0.0) as u64;
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    let secs = total % 60;

    if days > 0 {
        format!("{days}d {hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{hours:02}:{mins:02}:{secs:02}")
    }
}
