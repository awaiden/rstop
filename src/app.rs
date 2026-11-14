use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::cli::{CliArgs, SortColumn};
use crate::collectors::{ProcessInfo, SystemSnapshot};

const PAGE_STEP: usize = 10;

/// Selectable POSIX signal entry shown in the signal modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalOption {
    pub name: &'static str,
    pub signal: i32,
}

impl SignalOption {
    /// Returns a formatted label such as `"SIGTERM (15)"`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.signal)
    }
}

/// Ordered list of signals available in `SignalModalState`.
pub const SIGNAL_OPTIONS: [SignalOption; 6] = [
    SignalOption {
        name: "SIGTERM",
        signal: 15,
    },
    SignalOption {
        name: "SIGKILL",
        signal: 9,
    },
    SignalOption {
        name: "SIGHUP",
        signal: 1,
    },
    SignalOption {
        name: "SIGINT",
        signal: 2,
    },
    SignalOption {
        name: "SIGSTOP",
        signal: 19,
    },
    SignalOption {
        name: "SIGCONT",
        signal: 18,
    },
];

/// State for the send-signal confirmation modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalModalState {
    pub pid: i32,
    pub comm: String,
    pub selected_signal_idx: usize,
}

impl SignalModalState {
    /// Creates a new signal modal targeting `pid` (`comm`), defaulting to `SIGTERM (15)`.
    pub fn new(pid: i32, comm: String) -> Self {
        Self {
            pid,
            comm,
            selected_signal_idx: 0,
        }
    }

    /// Returns all selectable signal options.
    pub fn options(&self) -> &'static [SignalOption] {
        &SIGNAL_OPTIONS
    }

    /// Returns the currently highlighted signal option.
    pub fn selected_signal(&self) -> SignalOption {
        SIGNAL_OPTIONS
            .get(self.selected_signal_idx)
            .copied()
            .unwrap_or(SIGNAL_OPTIONS[0])
    }
}

/// Active input / overlay mode of the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
    SignalModal(SignalModalState),
    Help,
}

/// Side-effect requested by `AppState::handle_key` for the runtime loop to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    None,
    Quit,
    SendSignal { pid: i32, signal: i32 },
    RefreshNow,
}

/// A single visible row in the process table, enriched with tree metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRow {
    pub process: ProcessInfo,
    pub tree_depth: usize,
    pub is_last_child: bool,
    pub has_children: bool,
    pub is_collapsed: bool,
}

/// Complete UI and interaction state for `rstop`.
#[derive(Debug, Clone)]
pub struct AppState {
    pub snapshot: Option<SystemSnapshot>,
    pub visible_rows: Vec<ProcessRow>,
    pub selected_pid: Option<i32>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub sort_col: SortColumn,
    pub sort_desc: bool,
    pub tree_mode: bool,
    pub collapsed_pids: HashSet<i32>,
    pub filter_text: String,
    pub user_filter: Option<String>,
    pub input_mode: InputMode,
    pub status_message: Option<(String, Instant)>,
    pub delay_ms: u64,
    pub no_color: bool,
    pub should_quit: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            snapshot: None,
            visible_rows: Vec::new(),
            selected_pid: None,
            selected_index: 0,
            scroll_offset: 0,
            sort_col: SortColumn::Cpu,
            sort_desc: true,
            tree_mode: false,
            collapsed_pids: HashSet::new(),
            filter_text: String::new(),
            user_filter: None,
            input_mode: InputMode::Normal,
            status_message: None,
            delay_ms: 1000,
            no_color: false,
            should_quit: false,
        }
    }
}

impl AppState {
    /// Constructs an `AppState` initialized from parsed CLI arguments.
    pub fn from_cli(args: &CliArgs) -> Self {
        Self {
            sort_col: args.sort,
            sort_desc: default_sort_desc(args.sort),
            tree_mode: args.tree,
            filter_text: args.filter.clone().unwrap_or_default(),
            user_filter: args.user.clone(),
            delay_ms: args.delay,
            no_color: args.no_color,
            ..Self::default()
        }
    }

    /// Replaces the current system snapshot and rebuilds the visible process rows.
    pub fn update_snapshot(&mut self, snapshot: SystemSnapshot) {
        self.snapshot = Some(snapshot);
        self.rebuild_view();
    }

    /// Sets a transient status message stamped with the current instant.
    pub fn set_status_message(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now()));
    }

    /// Clears `status_message` if it has been displayed for at least `ttl`.
    pub fn clear_expired_status(&mut self, ttl: Duration) {
        if let Some((_, created_at)) = &self.status_message
            && created_at.elapsed() >= ttl
        {
            self.status_message = None;
        }
    }

    /// Adjusts `scroll_offset` so `selected_index` stays inside a viewport of `viewport_height` rows.
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 || self.visible_rows.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + viewport_height {
            self.scroll_offset = self.selected_index + 1 - viewport_height;
        }
        let max_offset = self.visible_rows.len().saturating_sub(viewport_height);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    /// Recomputes `visible_rows` from `snapshot` using active filters, sort order, and tree mode,
    /// preserving `selected_pid` when possible.
    pub fn rebuild_view(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            self.visible_rows.clear();
            self.selected_index = 0;
            self.selected_pid = None;
            self.scroll_offset = 0;
            return;
        };

        let user_filter = self.user_filter.as_deref();
        let filter_lower = (!self.filter_text.is_empty()).then(|| self.filter_text.to_lowercase());
        let mut pid_buf = String::with_capacity(16);

        let mut filtered: Vec<&ProcessInfo> = snapshot
            .processes
            .iter()
            .filter(|proc_info| {
                if let Some(target_user) = user_filter
                    && !eq_ignore_case(&proc_info.user, target_user)
                {
                    return false;
                }
                if let Some(ref needle) = filter_lower {
                    use std::fmt::Write;
                    pid_buf.clear();
                    let _ = write!(pid_buf, "{}", proc_info.pid);
                    let matches = contains_ignore_case(&proc_info.comm, needle)
                        || contains_ignore_case(&proc_info.cmdline, needle)
                        || contains_ignore_case(&proc_info.user, needle)
                        || pid_buf.contains(needle.as_str());
                    if !matches {
                        return false;
                    }
                }
                true
            })
            .collect();

        if self.tree_mode {
            self.visible_rows = build_tree_rows(
                &filtered,
                self.sort_col,
                self.sort_desc,
                &self.collapsed_pids,
            );
        } else {
            filtered.sort_by(|a, b| compare_processes(a, b, self.sort_col, self.sort_desc));
            self.visible_rows = filtered
                .into_iter()
                .map(|proc_info| ProcessRow {
                    process: proc_info.clone(),
                    tree_depth: 0,
                    is_last_child: false,
                    has_children: false,
                    is_collapsed: false,
                })
                .collect();
        }

        self.reconcile_selection();
    }

    /// Processes a terminal key event according to the current `InputMode` and returns an `AppAction`.
    pub fn handle_key(&mut self, key: KeyEvent) -> AppAction {
        if key.kind == KeyEventKind::Release {
            return AppAction::None;
        }

        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::Filter => self.handle_filter_key(key),
            InputMode::SignalModal(_) => self.handle_signal_modal_key(key),
            InputMode::Help => self.handle_help_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> AppAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            self.should_quit = true;
            return AppAction::Quit;
        }

        if ctrl || alt {
            return AppAction::None;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::F(10) => {
                self.should_quit = true;
                AppAction::Quit
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection_by(-1);
                AppAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection_by(1);
                AppAction::None
            }
            KeyCode::PageUp => {
                self.move_selection_by(-(PAGE_STEP as isize));
                AppAction::None
            }
            KeyCode::PageDown => {
                self.move_selection_by(PAGE_STEP as isize);
                AppAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.select_index(0);
                AppAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                let last = self.visible_rows.len().saturating_sub(1);
                self.select_index(last);
                AppAction::None
            }
            KeyCode::Char('/') | KeyCode::F(3) => {
                self.input_mode = InputMode::Filter;
                AppAction::None
            }
            KeyCode::Char('t') | KeyCode::F(5) => {
                self.tree_mode = !self.tree_mode;
                self.rebuild_view();
                AppAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.tree_mode => {
                if let Some(row) = self.visible_rows.get(self.selected_index) {
                    let pid = row.process.pid;
                    if row.has_children {
                        if !self.collapsed_pids.remove(&pid) {
                            self.collapsed_pids.insert(pid);
                        }
                        self.rebuild_view();
                    }
                }
                AppAction::None
            }
            KeyCode::Char('s') | KeyCode::Char('>') => {
                self.sort_col = next_sort_column(self.sort_col);
                self.rebuild_view();
                AppAction::None
            }
            KeyCode::Char('<') => {
                self.sort_col = prev_sort_column(self.sort_col);
                self.rebuild_view();
                AppAction::None
            }
            KeyCode::Char('r') | KeyCode::Char('I') => {
                self.sort_desc = !self.sort_desc;
                self.rebuild_view();
                AppAction::None
            }
            KeyCode::F(9) | KeyCode::Char('x') | KeyCode::Char('K') => {
                if let Some(row) = self.visible_rows.get(self.selected_index) {
                    self.input_mode = InputMode::SignalModal(SignalModalState::new(
                        row.process.pid,
                        row.process.comm.clone(),
                    ));
                }
                AppAction::None
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.input_mode = InputMode::Help;
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.filter_text.clear();
                self.input_mode = InputMode::Normal;
                self.rebuild_view();
            }
            KeyCode::Backspace => {
                if self.filter_text.pop().is_some() {
                    self.rebuild_view();
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.filter_text.push(c);
                self.rebuild_view();
            }
            _ => {}
        }
        AppAction::None
    }

    fn handle_signal_modal_key(&mut self, key: KeyEvent) -> AppAction {
        let InputMode::SignalModal(ref mut modal) = self.input_mode else {
            return AppAction::None;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                modal.selected_signal_idx = modal.selected_signal_idx.saturating_sub(1);
                AppAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_idx = SIGNAL_OPTIONS.len().saturating_sub(1);
                modal.selected_signal_idx = (modal.selected_signal_idx + 1).min(max_idx);
                AppAction::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let pid = modal.pid;
                let signal = modal.selected_signal().signal;
                self.input_mode = InputMode::Normal;
                AppAction::SendSignal { pid, signal }
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                self.input_mode = InputMode::Normal;
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Esc
            | KeyCode::Char('?')
            | KeyCode::Char('q')
            | KeyCode::Enter
            | KeyCode::F(1) => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
        AppAction::None
    }

    fn move_selection_by(&mut self, delta: isize) {
        if self.visible_rows.is_empty() {
            self.selected_index = 0;
            self.selected_pid = None;
            return;
        }
        let max_idx = self.visible_rows.len() - 1;
        let next = if delta < 0 {
            self.selected_index.saturating_sub(delta.unsigned_abs())
        } else {
            self.selected_index
                .saturating_add(delta as usize)
                .min(max_idx)
        };
        self.select_index(next);
    }

    fn select_index(&mut self, index: usize) {
        if self.visible_rows.is_empty() {
            self.selected_index = 0;
            self.selected_pid = None;
            return;
        }
        let clamped = index.min(self.visible_rows.len() - 1);
        self.selected_index = clamped;
        self.selected_pid = Some(self.visible_rows[clamped].process.pid);
    }

    fn reconcile_selection(&mut self) {
        if self.visible_rows.is_empty() {
            self.selected_index = 0;
            self.selected_pid = None;
            self.scroll_offset = 0;
            return;
        }

        if let Some(pid) = self.selected_pid
            && let Some(pos) = self.visible_rows.iter().position(|r| r.process.pid == pid)
        {
            self.selected_index = pos;
        } else {
            self.selected_index = self.selected_index.min(self.visible_rows.len() - 1);
            self.selected_pid = Some(self.visible_rows[self.selected_index].process.pid);
        }

        if self.scroll_offset >= self.visible_rows.len() {
            self.scroll_offset = self.visible_rows.len() - 1;
        }
    }
}

fn default_sort_desc(col: SortColumn) -> bool {
    matches!(col, SortColumn::Cpu | SortColumn::Mem | SortColumn::Time)
}

fn next_sort_column(col: SortColumn) -> SortColumn {
    match col {
        SortColumn::Cpu => SortColumn::Mem,
        SortColumn::Mem => SortColumn::Pid,
        SortColumn::Pid => SortColumn::User,
        SortColumn::User => SortColumn::Time,
        SortColumn::Time => SortColumn::Command,
        SortColumn::Command => SortColumn::Cpu,
    }
}

fn prev_sort_column(col: SortColumn) -> SortColumn {
    match col {
        SortColumn::Cpu => SortColumn::Command,
        SortColumn::Mem => SortColumn::Cpu,
        SortColumn::Pid => SortColumn::Mem,
        SortColumn::User => SortColumn::Pid,
        SortColumn::Time => SortColumn::User,
        SortColumn::Command => SortColumn::Time,
    }
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
    }
}

fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle_lower.as_bytes();
        return h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    haystack.to_lowercase().contains(needle_lower)
}

fn cmp_ignore_case_then_exact(a: &str, b: &str) -> Ordering {
    let ci = if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    };
    ci.then_with(|| a.cmp(b))
}

fn compare_processes(a: &ProcessInfo, b: &ProcessInfo, col: SortColumn, desc: bool) -> Ordering {
    let primary = match col {
        SortColumn::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
        SortColumn::Mem => a.mem_pct.total_cmp(&b.mem_pct),
        SortColumn::Pid => a.pid.cmp(&b.pid),
        SortColumn::User => cmp_ignore_case_then_exact(&a.user, &b.user),
        SortColumn::Time => a.cpu_time_ticks.cmp(&b.cpu_time_ticks),
        SortColumn::Command => cmp_ignore_case_then_exact(&a.comm, &b.comm),
    };

    let directed = if desc { primary.reverse() } else { primary };
    directed.then_with(|| a.pid.cmp(&b.pid))
}

fn build_tree_rows(
    processes: &[&ProcessInfo],
    sort_col: SortColumn,
    sort_desc: bool,
    collapsed_pids: &HashSet<i32>,
) -> Vec<ProcessRow> {
    let present_pids: HashSet<i32> = processes.iter().map(|p| p.pid).collect();
    let mut roots: Vec<&ProcessInfo> = Vec::new();
    let mut children_map: HashMap<i32, Vec<&ProcessInfo>> = HashMap::new();

    for &proc_info in processes {
        if proc_info.ppid == 0
            || proc_info.ppid == proc_info.pid
            || !present_pids.contains(&proc_info.ppid)
        {
            roots.push(proc_info);
        } else {
            children_map
                .entry(proc_info.ppid)
                .or_default()
                .push(proc_info);
        }
    }

    roots.sort_by(|a, b| compare_processes(a, b, sort_col, sort_desc));
    for children in children_map.values_mut() {
        children.sort_by(|a, b| compare_processes(a, b, sort_col, sort_desc));
    }

    let mut rows = Vec::with_capacity(processes.len());
    let mut visited = HashSet::with_capacity(processes.len());

    fn dfs(
        siblings: &[&ProcessInfo],
        depth: usize,
        children_map: &HashMap<i32, Vec<&ProcessInfo>>,
        collapsed_pids: &HashSet<i32>,
        visited: &mut HashSet<i32>,
        rows: &mut Vec<ProcessRow>,
    ) {
        for (idx, &proc_info) in siblings.iter().enumerate() {
            if !visited.insert(proc_info.pid) {
                continue;
            }
            let children = children_map.get(&proc_info.pid);
            let has_children = children.is_some_and(|c| !c.is_empty());
            let is_collapsed = has_children && collapsed_pids.contains(&proc_info.pid);
            let is_last_child = idx + 1 == siblings.len();

            rows.push(ProcessRow {
                process: proc_info.clone(),
                tree_depth: depth,
                is_last_child,
                has_children,
                is_collapsed,
            });

            if has_children
                && !is_collapsed
                && let Some(child_list) = children
            {
                dfs(
                    child_list,
                    depth + 1,
                    children_map,
                    collapsed_pids,
                    visited,
                    rows,
                );
            }
        }
    }

    dfs(
        &roots,
        0,
        &children_map,
        collapsed_pids,
        &mut visited,
        &mut rows,
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::{
        CpuStats, CpuUsage, HistoryBuffer, LoadUptime, MemStats, ProcessState,
    };

    #[allow(clippy::too_many_arguments)]
    fn make_proc(
        pid: i32,
        ppid: i32,
        user: &str,
        comm: &str,
        cmdline: &str,
        cpu_pct: f64,
        mem_pct: f64,
        cpu_time_ticks: u64,
    ) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            uid: 1000,
            user: user.to_string(),
            state: ProcessState::Sleeping,
            priority: 20,
            nice: 0,
            threads: 1,
            rss_bytes: 1024,
            vsize_bytes: 4096,
            mem_pct,
            cpu_pct,
            cpu_time_ticks,
            comm: comm.to_string(),
            cmdline: cmdline.to_string(),
        }
    }

    fn make_snapshot(processes: Vec<ProcessInfo>) -> SystemSnapshot {
        SystemSnapshot {
            cpu: CpuStats {
                total: CpuUsage::default(),
                cores: Vec::new(),
                history: HistoryBuffer::new(10),
            },
            mem: MemStats::default(),
            load: LoadUptime::default(),
            net: Vec::new(),
            processes,
            total_cpu_delta_ticks: 100,
            num_cores: 4,
            collected_at: Instant::now(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_processes() -> Vec<ProcessInfo> {
        vec![
            make_proc(
                10,
                1,
                "alice",
                "zebra",
                "/usr/bin/zebra --fast",
                12.5,
                4.0,
                300,
            ),
            make_proc(2, 0, "root", "alpha", "/sbin/alpha", 5.0, 20.0, 500),
            make_proc(5, 2, "bob", "Beta", "beta --daemon", 50.0, 1.5, 100),
            make_proc(7, 2, "alice", "gamma", "/opt/gamma", 12.5, 10.0, 300),
        ]
    }

    #[test]
    fn sorting_all_columns_asc_and_desc_with_pid_tiebreaker() {
        let mut state = AppState::default();
        state.update_snapshot(make_snapshot(sample_processes()));

        // Cpu desc: 5 (50.0), 7 & 10 (12.5 -> pid tiebreaker asc: 7 then 10), 2 (5.0)
        state.sort_col = SortColumn::Cpu;
        state.sort_desc = true;
        state.rebuild_view();
        let pids: Vec<i32> = state.visible_rows.iter().map(|r| r.process.pid).collect();
        assert_eq!(pids, vec![5, 7, 10, 2]);

        // Cpu asc: 2 (5.0), 7 & 10 (12.5 -> 7 then 10), 5 (50.0)
        state.sort_desc = false;
        state.rebuild_view();
        let pids: Vec<i32> = state.visible_rows.iter().map(|r| r.process.pid).collect();
        assert_eq!(pids, vec![2, 7, 10, 5]);

        // Mem desc: 2 (20.0), 7 (10.0), 10 (4.0), 5 (1.5)
        state.sort_col = SortColumn::Mem;
        state.sort_desc = true;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![2, 7, 10, 5]
        );

        // Mem asc: 5, 10, 7, 2
        state.sort_desc = false;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![5, 10, 7, 2]
        );

        // Pid asc & desc
        state.sort_col = SortColumn::Pid;
        state.sort_desc = false;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![2, 5, 7, 10]
        );
        state.sort_desc = true;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![10, 7, 5, 2]
        );

        // User asc (alice (pid 7, 10), bob (5), root (2)) & desc
        state.sort_col = SortColumn::User;
        state.sort_desc = false;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![7, 10, 5, 2]
        );
        state.sort_desc = true;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![2, 5, 7, 10]
        );

        // Time desc (500->2, 300->7,10, 100->5) & asc
        state.sort_col = SortColumn::Time;
        state.sort_desc = true;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![2, 7, 10, 5]
        );
        state.sort_desc = false;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![5, 7, 10, 2]
        );

        // Command asc (alpha->2, Beta->5, gamma->7, zebra->10) & desc
        state.sort_col = SortColumn::Command;
        state.sort_desc = false;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![2, 5, 7, 10]
        );
        state.sort_desc = true;
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![10, 7, 5, 2]
        );
    }

    #[test]
    fn filtering_by_user_and_text_across_fields() {
        let mut state = AppState {
            sort_col: SortColumn::Pid,
            sort_desc: false,
            ..Default::default()
        };
        state.update_snapshot(make_snapshot(sample_processes()));

        // Exact case-insensitive user filter
        state.user_filter = Some("ALICE".to_string());
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![7, 10]
        );

        // Partial user name should not match exact user_filter
        state.user_filter = Some("ali".to_string());
        state.rebuild_view();
        assert!(state.visible_rows.is_empty());

        // Clear user filter, test filter_text against comm, cmdline, user, pid
        state.user_filter = None;

        // Match cmdline (--daemon)
        state.filter_text = "DAEMON".to_string();
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![5]
        );

        // Match pid ("10")
        state.filter_text = "10".to_string();
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![10]
        );

        // Match user ("bob")
        state.filter_text = "BOB".to_string();
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![5]
        );

        // Combined user_filter + filter_text
        state.user_filter = Some("alice".to_string());
        state.filter_text = "fast".to_string();
        state.rebuild_view();
        assert_eq!(
            state
                .visible_rows
                .iter()
                .map(|r| r.process.pid)
                .collect::<Vec<_>>(),
            vec![10]
        );
    }

    #[test]
    fn tree_mode_ordering_depth_and_collapse_expand() {
        let procs = vec![
            make_proc(1, 0, "root", "systemd", "/sbin/init", 1.0, 1.0, 100),
            make_proc(100, 1, "root", "sshd", "sshd", 2.0, 1.0, 50),
            make_proc(200, 100, "alice", "bash", "-bash", 5.0, 2.0, 20),
            make_proc(201, 100, "bob", "zsh", "-zsh", 10.0, 2.0, 30),
            make_proc(300, 1, "root", "cron", "crond", 15.0, 1.0, 40),
            make_proc(400, 999, "root", "orphan", "orphan", 0.5, 0.5, 10), // ppid 999 absent -> root
        ];

        let mut state = AppState {
            tree_mode: true,
            sort_col: SortColumn::Cpu,
            sort_desc: true,
            ..Default::default()
        };
        state.update_snapshot(make_snapshot(procs));

        // Roots: 1 (1.0) and 400 (0.5) -> 1 first, then its children sorted by Cpu desc: 300 (15.0), 100 (2.0),
        // then 100's children: 201 (10.0), 200 (5.0), then root 400.
        let summary: Vec<(i32, usize, bool, bool, bool)> = state
            .visible_rows
            .iter()
            .map(|r| {
                (
                    r.process.pid,
                    r.tree_depth,
                    r.has_children,
                    r.is_collapsed,
                    r.is_last_child,
                )
            })
            .collect();

        assert_eq!(
            summary,
            vec![
                (1, 0, true, false, false),
                (300, 1, false, false, false),
                (100, 1, true, false, true),
                (201, 2, false, false, false),
                (200, 2, false, false, true),
                (400, 0, false, false, true),
            ]
        );

        // Select pid 100 (index 2) and collapse with Space
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected_pid, Some(100));
        state.handle_key(key(KeyCode::Char(' ')));

        let pids_collapsed: Vec<i32> = state.visible_rows.iter().map(|r| r.process.pid).collect();
        assert_eq!(pids_collapsed, vec![1, 300, 100, 400]);
        assert!(state.visible_rows[2].is_collapsed);

        // Expand again with Enter
        state.handle_key(key(KeyCode::Enter));
        let pids_expanded: Vec<i32> = state.visible_rows.iter().map(|r| r.process.pid).collect();
        assert_eq!(pids_expanded, vec![1, 300, 100, 201, 200, 400]);
        assert!(!state.visible_rows[2].is_collapsed);
    }

    #[test]
    fn selection_preserved_across_snapshots_and_clamped_on_removal() {
        let mut state = AppState {
            sort_col: SortColumn::Cpu,
            sort_desc: true,
            ..Default::default()
        };

        state.update_snapshot(make_snapshot(vec![
            make_proc(1, 0, "root", "a", "a", 30.0, 1.0, 10),
            make_proc(2, 0, "root", "b", "b", 20.0, 1.0, 10),
            make_proc(3, 0, "root", "c", "c", 10.0, 1.0, 10),
        ]));

        // Select pid 2 at index 1
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected_pid, Some(2));
        assert_eq!(state.selected_index, 1);

        // Refresh where pid 2 jumps to top (cpu 90.0)
        state.update_snapshot(make_snapshot(vec![
            make_proc(1, 0, "root", "a", "a", 30.0, 1.0, 10),
            make_proc(2, 0, "root", "b", "b", 90.0, 1.0, 10),
            make_proc(3, 0, "root", "c", "c", 10.0, 1.0, 10),
        ]));
        assert_eq!(state.selected_pid, Some(2));
        assert_eq!(state.selected_index, 0);

        // Move to last item (pid 3 at index 2), then refresh without pid 3 -> clamped to index 1
        state.handle_key(key(KeyCode::End));
        assert_eq!(state.selected_pid, Some(3));
        assert_eq!(state.selected_index, 2);

        state.update_snapshot(make_snapshot(vec![
            make_proc(2, 0, "root", "b", "b", 90.0, 1.0, 10),
            make_proc(1, 0, "root", "a", "a", 30.0, 1.0, 10),
        ]));
        assert_eq!(state.selected_index, 1);
        assert_eq!(state.selected_pid, Some(1));

        // Empty snapshot resets selection
        state.update_snapshot(make_snapshot(Vec::new()));
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.selected_pid, None);
    }

    #[test]
    fn normal_mode_navigation_sort_tree_and_quit_keybindings() {
        let mut procs = Vec::new();
        for i in 1..=25 {
            procs.push(make_proc(
                i,
                0,
                "root",
                &format!("p{i}"),
                "cmd",
                (26 - i) as f64,
                1.0,
                i as u64,
            ));
        }

        let mut state = AppState::default();
        state.update_snapshot(make_snapshot(procs));
        assert_eq!(state.selected_index, 0);

        // Down/j and Up/k
        state.handle_key(key(KeyCode::Char('j')));
        assert_eq!(state.selected_index, 1);
        state.handle_key(key(KeyCode::Char('k')));
        assert_eq!(state.selected_index, 0);

        // PageDown / PageUp / End / Home
        state.handle_key(key(KeyCode::PageDown));
        assert_eq!(state.selected_index, PAGE_STEP);
        state.handle_key(key(KeyCode::PageUp));
        assert_eq!(state.selected_index, 0);
        state.handle_key(key(KeyCode::Char('G')));
        assert_eq!(state.selected_index, 24);
        state.handle_key(key(KeyCode::Char('g')));
        assert_eq!(state.selected_index, 0);

        // Cycle sort column with s, >, < and reverse with r / I
        assert_eq!(state.sort_col, SortColumn::Cpu);
        state.handle_key(key(KeyCode::Char('s')));
        assert_eq!(state.sort_col, SortColumn::Mem);
        state.handle_key(key(KeyCode::Char('>')));
        assert_eq!(state.sort_col, SortColumn::Pid);
        state.handle_key(key(KeyCode::Char('<')));
        assert_eq!(state.sort_col, SortColumn::Mem);

        assert!(state.sort_desc);
        state.handle_key(key(KeyCode::Char('r')));
        assert!(!state.sort_desc);
        state.handle_key(key(KeyCode::Char('I')));
        assert!(state.sort_desc);

        // Toggle tree mode with t and F5
        assert!(!state.tree_mode);
        state.handle_key(key(KeyCode::Char('t')));
        assert!(state.tree_mode);
        state.handle_key(key(KeyCode::F(5)));
        assert!(!state.tree_mode);

        // Quit with q, F10, Ctrl+c
        assert_eq!(state.handle_key(key(KeyCode::Char('q'))), AppAction::Quit);
        assert!(state.should_quit);

        state.should_quit = false;
        assert_eq!(state.handle_key(key(KeyCode::F(10))), AppAction::Quit);
        assert!(state.should_quit);

        state.should_quit = false;
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(ctrl_c), AppAction::Quit);
        assert!(state.should_quit);
    }

    #[test]
    fn filter_signal_modal_and_help_mode_transitions() {
        let mut state = AppState::default();
        state.update_snapshot(make_snapshot(sample_processes()));

        // Enter Filter mode via '/'
        state.handle_key(key(KeyCode::Char('/')));
        assert_eq!(state.input_mode, InputMode::Filter);

        state.handle_key(key(KeyCode::Char('a')));
        state.handle_key(key(KeyCode::Char('l')));
        state.handle_key(key(KeyCode::Char('p')));
        assert_eq!(state.filter_text, "alp");
        assert_eq!(state.visible_rows.len(), 1);
        assert_eq!(state.visible_rows[0].process.pid, 2);

        // Backspace + Enter confirms filter and returns to Normal
        state.handle_key(key(KeyCode::Backspace));
        assert_eq!(state.filter_text, "al");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.input_mode, InputMode::Normal);
        assert_eq!(state.filter_text, "al");

        // Re-enter Filter via F3 and Esc clears filter
        state.handle_key(key(KeyCode::F(3)));
        assert_eq!(state.input_mode, InputMode::Filter);
        state.handle_key(key(KeyCode::Esc));
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.filter_text.is_empty());
        assert_eq!(state.visible_rows.len(), 4);

        // Selection is preserved on pid 2 after clearing filter; open SignalModal via 'x'
        assert_eq!(state.selected_pid, Some(2));
        state.handle_key(key(KeyCode::Char('x')));
        assert!(matches!(state.input_mode, InputMode::SignalModal(_)));

        // Move down to SIGKILL (9) and confirm with 'y'
        state.handle_key(key(KeyCode::Down));
        let action = state.handle_key(key(KeyCode::Char('y')));
        assert_eq!(action, AppAction::SendSignal { pid: 2, signal: 9 });
        assert_eq!(state.input_mode, InputMode::Normal);

        // Open SignalModal via F9 and cancel with Esc
        state.handle_key(key(KeyCode::F(9)));
        assert!(matches!(state.input_mode, InputMode::SignalModal(_)));
        let cancel_action = state.handle_key(key(KeyCode::Esc));
        assert_eq!(cancel_action, AppAction::None);
        assert_eq!(state.input_mode, InputMode::Normal);

        // Open Help via '?' and close via 'q'
        state.handle_key(key(KeyCode::Char('?')));
        assert_eq!(state.input_mode, InputMode::Help);
        state.handle_key(key(KeyCode::Char('q')));
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(!state.should_quit);

        // Open Help via F1 and close via Esc
        state.handle_key(key(KeyCode::F(1)));
        assert_eq!(state.input_mode, InputMode::Help);
        state.handle_key(key(KeyCode::Esc));
        assert_eq!(state.input_mode, InputMode::Normal);
    }
}
