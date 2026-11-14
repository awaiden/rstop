pub mod header;
pub mod overlays;
pub mod process_table;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::AppState;

/// Color and style palette supporting both color and `--no-color` modes.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub no_color: bool,
    pub text: Style,
    pub muted: Style,
    pub title: Style,
    pub border: Style,
    pub accent: Style,
    pub table_header: Style,
    pub table_header_active: Style,
    pub selected_row: Style,
    pub status_bar: Style,
    pub status_alert: Style,
    pub proc_running: Style,
    pub proc_zombie: Style,
    pub usage_low: Style,
    pub usage_mid: Style,
    pub usage_high: Style,
}

impl Theme {
    /// Builds a `Theme` for the requested color mode.
    pub fn new(no_color: bool) -> Self {
        if no_color {
            Self {
                no_color: true,
                text: Style::default(),
                muted: Style::default(),
                title: Style::default().add_modifier(Modifier::BOLD),
                border: Style::default(),
                accent: Style::default().add_modifier(Modifier::BOLD),
                table_header: Style::default().add_modifier(Modifier::BOLD),
                table_header_active: Style::default()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                selected_row: Style::default().add_modifier(Modifier::REVERSED),
                status_bar: Style::default().add_modifier(Modifier::REVERSED),
                status_alert: Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
                proc_running: Style::default().add_modifier(Modifier::BOLD),
                proc_zombie: Style::default().add_modifier(Modifier::DIM),
                usage_low: Style::default(),
                usage_mid: Style::default(),
                usage_high: Style::default().add_modifier(Modifier::BOLD),
            }
        } else {
            Self {
                no_color: false,
                text: Style::default().fg(Color::White),
                muted: Style::default().fg(Color::DarkGray),
                title: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                border: Style::default().fg(Color::DarkGray),
                accent: Style::default().fg(Color::Cyan),
                table_header: Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
                table_header_active: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                selected_row: Style::default().bg(Color::DarkGray).fg(Color::White),
                status_bar: Style::default().bg(Color::DarkGray).fg(Color::White),
                status_alert: Style::default()
                    .bg(Color::Yellow)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                proc_running: Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                proc_zombie: Style::default().fg(Color::Red),
                usage_low: Style::default().fg(Color::Green),
                usage_mid: Style::default().fg(Color::Yellow),
                usage_high: Style::default().fg(Color::Red),
            }
        }
    }

    /// Returns a threshold-colored style for percentage usage bars (0..=100).
    pub fn usage_style(&self, pct: f64) -> Style {
        if pct >= 85.0 {
            self.usage_high
        } else if pct >= 50.0 {
            self.usage_mid
        } else {
            self.usage_low
        }
    }
}

/// Renders the complete `rstop` TUI frame: header, process table, footer, and active overlays.
pub fn render(frame: &mut Frame, app: &mut AppState) {
    let theme = Theme::new(app.no_color);
    let area = frame.area();
    let header_height = header::desired_height(app, area.width, area.height);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);

    header::render(frame, chunks[0], app, &theme);
    process_table::render(frame, chunks[1], app, &theme);
    render_footer(frame, chunks[2], app, &theme);
    overlays::render(frame, chunks[1], app, &theme);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &AppState, theme: &Theme) {
    if let Some((msg, _)) = &app.status_message {
        let line = Line::from(Span::styled(format!(" {msg}"), theme.status_alert));
        frame.render_widget(Paragraph::new(line).style(theme.status_alert), area);
        return;
    }

    let mut badges = Vec::new();
    if app.tree_mode {
        badges.push("[Tree]");
    }
    if let Some(user) = &app.user_filter {
        badges.push("[User:");
        badges.push(user.as_str());
        badges.push("]");
    }
    if !app.filter_text.is_empty() {
        badges.push("[Filter:");
        badges.push(app.filter_text.as_str());
        badges.push("]");
    }

    let badge_str = if badges.is_empty() {
        String::new()
    } else {
        format!(" {} ", badges.join(""))
    };

    let hints = " F1/?:Help  /:Filter  t/F5:Tree  s:Sort  r:Rev  x/F9:Signal  q/F10:Quit";
    let line = Line::from(vec![
        Span::styled(hints, theme.status_bar),
        Span::styled(badge_str, theme.accent.add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme.status_bar), area);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::app::{InputMode, SignalModalState};
    use crate::cli::SortColumn;
    use crate::collectors::{
        CpuStats, CpuUsage, HistoryBuffer, LoadUptime, MemStats, NetInterfaceStats, ProcessInfo,
        ProcessState, SystemSnapshot,
    };

    fn sample_snapshot() -> SystemSnapshot {
        let mut history = HistoryBuffer::new(16);
        history.push(15.0);
        history.push(28.0);
        history.push(42.5);

        SystemSnapshot {
            cpu: CpuStats {
                total: CpuUsage {
                    total_pct: 42.5,
                    user_pct: 30.0,
                    sys_pct: 10.5,
                    iowait_pct: 2.0,
                },
                cores: vec![
                    CpuUsage {
                        total_pct: 55.0,
                        user_pct: 40.0,
                        sys_pct: 15.0,
                        iowait_pct: 0.0,
                    },
                    CpuUsage {
                        total_pct: 30.0,
                        user_pct: 20.0,
                        sys_pct: 6.0,
                        iowait_pct: 4.0,
                    },
                ],
                history,
            },
            mem: MemStats {
                total_kb: 16 * 1024 * 1024,
                used_kb: 6 * 1024 * 1024,
                free_kb: 4 * 1024 * 1024,
                available_kb: 10 * 1024 * 1024,
                buffers_kb: 512 * 1024,
                cached_kb: 3 * 1024 * 1024,
                shared_kb: 128 * 1024,
                swap_total_kb: 4 * 1024 * 1024,
                swap_used_kb: 512 * 1024,
                swap_cached_kb: 0,
                swap_free_kb: 3_584 * 1024,
            },
            load: LoadUptime {
                load1: 1.25,
                load5: 0.90,
                load15: 0.75,
                uptime_secs: 93_785.0,
                running_threads: 2,
                total_threads: 140,
            },
            net: vec![NetInterfaceStats {
                name: "eth0".to_string(),
                rx_bytes: 1_000_000,
                tx_bytes: 500_000,
                rx_packets: 1000,
                tx_packets: 500,
                rx_bytes_per_sec: 1_572_864.0,
                tx_bytes_per_sec: 262_144.0,
            }],
            processes: vec![
                ProcessInfo {
                    pid: 1,
                    ppid: 0,
                    uid: 0,
                    user: "root".to_string(),
                    state: ProcessState::Sleeping,
                    priority: 20,
                    nice: 0,
                    threads: 1,
                    rss_bytes: 12 * 1024 * 1024,
                    vsize_bytes: 128 * 1024 * 1024,
                    mem_pct: 0.1,
                    cpu_pct: 0.5,
                    cpu_time_ticks: 1234,
                    comm: "systemd".to_string(),
                    cmdline: "/sbin/init".to_string(),
                },
                ProcessInfo {
                    pid: 100,
                    ppid: 1,
                    uid: 1000,
                    user: "alice".to_string(),
                    state: ProcessState::Running,
                    priority: 20,
                    nice: 0,
                    threads: 4,
                    rss_bytes: 256 * 1024 * 1024,
                    vsize_bytes: 1024 * 1024 * 1024,
                    mem_pct: 1.6,
                    cpu_pct: 35.2,
                    cpu_time_ticks: 98_765,
                    comm: "worker".to_string(),
                    cmdline: "/usr/bin/worker --jobs 4".to_string(),
                },
                ProcessInfo {
                    pid: 101,
                    ppid: 100,
                    uid: 1000,
                    user: "alice".to_string(),
                    state: ProcessState::Sleeping,
                    priority: 20,
                    nice: 0,
                    threads: 1,
                    rss_bytes: 32 * 1024 * 1024,
                    vsize_bytes: 64 * 1024 * 1024,
                    mem_pct: 0.2,
                    cpu_pct: 4.0,
                    cpu_time_ticks: 500,
                    comm: "helper".to_string(),
                    cmdline: "/usr/bin/helper".to_string(),
                },
                ProcessInfo {
                    pid: 102,
                    ppid: 1,
                    uid: 0,
                    user: "root".to_string(),
                    state: ProcessState::Sleeping,
                    priority: 20,
                    nice: 0,
                    threads: 1,
                    rss_bytes: 16 * 1024 * 1024,
                    vsize_bytes: 48 * 1024 * 1024,
                    mem_pct: 0.1,
                    cpu_pct: 0.2,
                    cpu_time_ticks: 210,
                    comm: "sshd".to_string(),
                    cmdline: "sshd: /usr/sbin/sshd".to_string(),
                },
            ],
            total_cpu_delta_ticks: 100,
            num_cores: 2,
            collected_at: Instant::now(),
        }
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_to_string(app: &mut AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, app))
            .expect("draw should succeed");
        buffer_to_string(terminal.backend().buffer())
    }

    #[test]
    fn renders_header_and_process_table_on_narrow_80x24_terminal() {
        let mut app = AppState {
            sort_col: SortColumn::Cpu,
            sort_desc: true,
            ..AppState::default()
        };
        app.update_snapshot(sample_snapshot());

        let rendered = render_to_string(&mut app, 80, 24);
        assert!(
            rendered.contains("CPU"),
            "expected CPU panel in:\n{rendered}"
        );
        assert!(
            rendered.contains("42.5%"),
            "expected total CPU% in:\n{rendered}"
        );
        assert!(
            rendered.contains("Mem "),
            "expected Mem gauge in:\n{rendered}"
        );
        assert!(
            rendered.contains("Swp "),
            "expected Swap gauge in:\n{rendered}"
        );
        assert!(
            rendered.contains("1.25 0.90 0.75"),
            "expected loadavg in:\n{rendered}"
        );
        assert!(
            rendered.contains("CPU%▼"),
            "expected sort indicator in:\n{rendered}"
        );
        assert!(
            rendered.contains("/usr/bin/wor"),
            "expected worker row in:\n{rendered}"
        );
        assert!(
            rendered.contains("F1/?:Help"),
            "expected footer hints in:\n{rendered}"
        );
    }

    #[test]
    fn renders_tree_indentation_and_collapse_indicators() {
        let mut app = AppState {
            tree_mode: true,
            ..AppState::default()
        };
        app.update_snapshot(sample_snapshot());

        let rendered = render_to_string(&mut app, 100, 28);
        assert!(
            rendered.contains("[-] /sbin/init"),
            "expected root expand marker:\n{rendered}"
        );
        assert!(
            rendered.contains("├─[-] /usr/bin/worker"),
            "expected child branch:\n{rendered}"
        );
        assert!(
            rendered.contains("│ └─/usr/bin/helper"),
            "expected grandchild branch:\n{rendered}"
        );
        assert!(
            rendered.contains("└─sshd:"),
            "expected last sibling branch:\n{rendered}"
        );

        // Collapse PID 100 and verify [+] marker
        app.collapsed_pids.insert(100);
        app.rebuild_view();
        let collapsed_rendered = render_to_string(&mut app, 100, 28);
        assert!(
            collapsed_rendered.contains("[+] /usr/bin/worker"),
            "expected collapsed marker:\n{collapsed_rendered}"
        );
        assert!(
            !collapsed_rendered.contains("/usr/bin/helper"),
            "collapsed child should be hidden:\n{collapsed_rendered}"
        );
    }

    #[test]
    fn renders_help_overlay_and_signal_modal_and_filter_bar() {
        let mut app = AppState::default();
        app.update_snapshot(sample_snapshot());

        app.input_mode = InputMode::Help;
        let help_out = render_to_string(&mut app, 80, 24);
        assert!(
            help_out.contains("Keybindings Help"),
            "expected help title:\n{help_out}"
        );
        assert!(
            help_out.contains("Toggle process tree view"),
            "expected help item:\n{help_out}"
        );

        app.input_mode = InputMode::SignalModal(SignalModalState::new(100, "worker".to_string()));
        let sig_out = render_to_string(&mut app, 80, 24);
        assert!(
            sig_out.contains("Send Signal"),
            "expected signal modal title:\n{sig_out}"
        );
        assert!(
            sig_out.contains("PID 100 (worker)"),
            "expected target pid:\n{sig_out}"
        );
        assert!(
            sig_out.contains("SIGTERM (15)"),
            "expected SIGTERM option:\n{sig_out}"
        );
        assert!(
            sig_out.contains("SIGKILL (9)"),
            "expected SIGKILL option:\n{sig_out}"
        );

        app.input_mode = InputMode::Filter;
        app.filter_text = "work".to_string();
        app.rebuild_view();
        let filter_out = render_to_string(&mut app, 80, 24);
        assert!(
            filter_out.contains("Filter: work_"),
            "expected filter bar:\n{filter_out}"
        );
    }

    #[test]
    fn renders_no_color_mode_and_empty_snapshot_without_panic() {
        let mut app = AppState {
            no_color: true,
            ..AppState::default()
        };
        let empty_out = render_to_string(&mut app, 80, 24);
        assert!(empty_out.contains("Collecting CPU metrics..."));

        app.update_snapshot(sample_snapshot());
        app.set_status_message("Sent signal 15 to PID 100");
        let with_status = render_to_string(&mut app, 80, 24);
        assert!(with_status.contains("Sent signal 15 to PID 100"));
    }
}
