use std::io::{self, Stdout};
use std::panic;
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
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
        terminal.draw(|frame| ui::render(frame, app))?;

        let timeout = next_tick.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => match app.handle_key(key) {
                    AppAction::Quit => break,
                    AppAction::SendSignal { pid, signal } => match send_signal(pid, signal) {
                        Ok(()) => {
                            app.set_status_message(format!("Sent signal {signal} to PID {pid}"));
                        }
                        Err(err) => {
                            app.set_status_message(format!(
                                "Failed to send signal {signal} to PID {pid}: {err}"
                            ));
                        }
                    },
                    AppAction::RefreshNow => {
                        collect_into_app(collector, app);
                        next_tick = Instant::now() + tick_rate;
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
            app.clear_expired_status(STATUS_TTL);
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
