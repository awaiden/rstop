use std::fs;
use std::io::{self, Stdout};
use std::panic;
use std::path::Path;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::cursor::Show;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use rstop::app::{AppAction, AppState};
use rstop::cli::CliArgs;
use rstop::collectors::SystemCollector;
use rstop::collectors::process::parse_proc_pid_stat;
use rstop::ui;

const HISTORY_CAPACITY: usize = 120;
const STATUS_TTL: Duration = Duration::from_secs(5);

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = match Terminal::new(backend) {
            Ok(term) => term,
            Err(err) => {
                let _ = restore_terminal();
                return Err(err);
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore_terminal();
        let _ = self.terminal.show_cursor();
    }
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    Ok(())
}

fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        default_hook(panic_info);
    }));
}

fn send_signal(pid: i32, signal: i32) -> io::Result<()> {
    if pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pid must be positive",
        ));
    }
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn verify_process_comm(proc_root: &Path, pid: i32, expected_comm: &str) -> io::Result<()> {
    if pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pid must be positive",
        ));
    }
    let stat_path = proc_root.join(pid.to_string()).join("stat");
    let content = fs::read_to_string(&stat_path)?;
    let stat = parse_proc_pid_stat(&content)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    if stat.comm != expected_comm {
        return Err(io::Error::other(format!(
            "process {pid} changed (expected '{expected_comm}', found '{}')",
            stat.comm
        )));
    }
    Ok(())
}

fn send_signal_verified(pid: i32, signal: i32, expected_comm: &str) -> io::Result<()> {
    verify_process_comm(Path::new("/proc"), pid, expected_comm)?;
    send_signal(pid, signal)
}

fn collect_into_app(collector: &mut SystemCollector, app: &mut AppState) {
    match collector.collect() {
        Ok(snapshot) => app.update_snapshot(snapshot),
        Err(err) => app.set_status_message(format!("Collection error: {err}")),
    }
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut AppState,
    collector: &mut SystemCollector,
) -> io::Result<()> {
    let tick_rate = Duration::from_millis(app.delay_ms);
    let mut next_tick = Instant::now() + tick_rate;

    collect_into_app(collector, app);

    while !app.should_quit {
        app.clear_expired_status(STATUS_TTL);
        terminal.draw(|frame| ui::render(frame, app))?;

        let mut timeout = next_tick.saturating_duration_since(Instant::now());
        if let Some((_, created_at)) = &app.status_message {
            let remaining = STATUS_TTL.saturating_sub(created_at.elapsed());
            timeout = timeout.min(remaining);
        }
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => match app.handle_key(key) {
                    AppAction::Quit => break,
                    AppAction::SendSignal { pid, signal, comm } => {
                        match send_signal_verified(pid, signal, &comm) {
                            Ok(()) => {
                                app.set_status_message(format!(
                                    "Sent signal {signal} to PID {pid}"
                                ));
                            }
                            Err(err) => {
                                app.set_status_message(format!(
                                    "Failed to send signal {signal} to PID {pid}: {err}"
                                ));
                            }
                        }
                    }
                    AppAction::None => {}
                },
                Event::Resize(_, _) => {
                    // Redraws on the next loop iteration with updated terminal dimensions.
                }
                _ => {}
            }
        }

        let now = Instant::now();
        if now >= next_tick {
            collect_into_app(collector, app);
            next_tick = now + tick_rate;
        }
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let args = CliArgs::parse();
    let mut app = AppState::from_cli(&args);
    let mut collector = SystemCollector::new(HISTORY_CAPACITY);

    install_panic_hook();
    let mut guard = TerminalGuard::new()?;
    run_event_loop(&mut guard.terminal, &mut app, &mut collector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_proc_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rstop_main_test_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn send_signal_rejects_non_positive_pids() {
        for bad_pid in [0, -1, -42] {
            let err = send_signal(bad_pid, libc::SIGTERM).expect_err("must reject pid <= 0");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn send_signal_returns_error_for_nonexistent_pid() {
        let err = send_signal(i32::MAX, 0).expect_err("non-existent pid should error");
        assert_eq!(err.raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn verify_process_comm_guards_against_pid_reuse() {
        let proc_root = make_temp_proc_dir("comm_guard");
        fs::create_dir_all(proc_root.join("42")).expect("create pid dir");
        fs::write(
            proc_root.join("42/stat"),
            "42 (new_proc) S 1 42 42 0 -1 0 0 0 0 0 1 0 0 0 20 0 1 0 10 1024 1 0\n",
        )
        .expect("write stat");

        assert!(verify_process_comm(&proc_root, 42, "new_proc").is_ok());

        let mismatch =
            verify_process_comm(&proc_root, 42, "old_proc").expect_err("mismatched comm must fail");
        assert!(mismatch.to_string().contains("changed"));

        let missing =
            verify_process_comm(&proc_root, 999, "old_proc").expect_err("missing pid must fail");
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);

        let _ = fs::remove_dir_all(&proc_root);
    }
}
