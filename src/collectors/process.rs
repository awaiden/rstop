use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::collectors::CollectorError;

/// Execution state of a Linux process from `/proc/[pid]/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep,
    Zombie,
    Stopped,
    TracingStop,
    Idle,
    Unknown(char),
}

impl ProcessState {
    /// Converts the single-character state code from `/proc/[pid]/stat` into `ProcessState`.
    pub fn from_char(c: char) -> Self {
        match c {
            'R' => Self::Running,
            'S' => Self::Sleeping,
            'D' => Self::DiskSleep,
            'Z' => Self::Zombie,
            'T' => Self::Stopped,
            't' => Self::TracingStop,
            'I' => Self::Idle,
            other => Self::Unknown(other),
        }
    }

    /// Returns the single-character representation of this process state.
    pub fn as_char(&self) -> char {
        match self {
            Self::Running => 'R',
            Self::Sleeping => 'S',
            Self::DiskSleep => 'D',
            Self::Zombie => 'Z',
            Self::Stopped => 'T',
            Self::TracingStop => 't',
            Self::Idle => 'I',
            Self::Unknown(c) => *c,
        }
    }
}

/// Raw fields parsed from `/proc/[pid]/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStatRaw {
    pub pid: i32,
    pub comm: String,
    pub state: ProcessState,
    pub ppid: i32,
    pub utime: u64,
    pub stime: u64,
    pub priority: i64,
    pub nice: i64,
    pub num_threads: i64,
    pub vsize: u64,
    pub rss_pages: u64,
}

/// Collected metrics and metadata for a single process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub user: String,
    pub state: ProcessState,
    pub priority: i64,
    pub nice: i64,
    pub rss_bytes: u64,
    pub vsize_bytes: u64,
    pub mem_pct: f64,
    pub cpu_pct: f64,
    pub cpu_time_ticks: u64,
    pub comm: String,
    pub cmdline: String,
}

/// Stateful collector for `/proc/[pid]` process entries.
#[derive(Debug)]
pub struct ProcessCollector {
    prev_ticks: HashMap<i32, u64>,
    uid_cache: HashMap<u32, String>,
    page_size: u64,
    clock_ticks_per_sec: u64,
}

impl Default for ProcessCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessCollector {
    /// Creates a new `ProcessCollector`, querying system page size, clock ticks, and `/etc/passwd`.
    pub fn new() -> Self {
        let uid_cache = fs::read_to_string("/etc/passwd")
            .map(|content| parse_passwd(&content))
            .unwrap_or_default();

        Self {
            prev_ticks: HashMap::new(),
            uid_cache,
            page_size: query_sysconf(libc::_SC_PAGESIZE, 4096),
            clock_ticks_per_sec: query_sysconf(libc::_SC_CLK_TCK, 100),
        }
    }

    /// Creates a `ProcessCollector` with an explicit UID cache, page size, and clock tick rate.
    pub fn with_config(
        uid_cache: HashMap<u32, String>,
        page_size: u64,
        clock_ticks_per_sec: u64,
    ) -> Self {
        Self {
            prev_ticks: HashMap::new(),
            uid_cache,
            page_size: page_size.max(1),
            clock_ticks_per_sec: clock_ticks_per_sec.max(1),
        }
    }

    /// Returns the system clock ticks per second (`_SC_CLK_TCK`).
    pub fn clock_ticks_per_sec(&self) -> u64 {
        self.clock_ticks_per_sec
    }

    /// Collects process information from `proc_root`.
    ///
    /// `cpu_pct` is normalized to single-core percentage (htop-style: 100% = 1 full core,
    /// up to `num_cores * 100%` for multi-threaded processes) using:
    /// `(curr_ticks - prev_ticks) / total_cpu_delta_ticks * 100.0 * num_cores.max(1)`.
    pub fn collect_from_proc(
        &mut self,
        proc_root: &Path,
        total_cpu_delta_ticks: u64,
        total_mem_kb: u64,
        num_cores: usize,
    ) -> Result<Vec<ProcessInfo>, CollectorError> {
        let entries = fs::read_dir(proc_root).map_err(|err| {
            CollectorError::Io(format!("failed to read {}: {err}", proc_root.display()))
        })?;

        let capacity_hint = self.prev_ticks.len();
        let mut next_ticks = HashMap::with_capacity(capacity_hint);
        let mut processes = Vec::with_capacity(capacity_hint);
        let total_mem_bytes = total_mem_kb.saturating_mul(1024);
        let core_scale = num_cores.max(1) as f64;
        let mut path_buf = PathBuf::with_capacity(64);
        let mut scratch = Vec::with_capacity(4096);

        for entry_res in entries {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) if is_vanished_process_error(&err) => continue,
                Err(err) => return Err(err.into()),
            };

            let file_name = entry.file_name();
            let Some(name_str) = file_name.to_str() else {
                continue;
            };
            let Ok(pid) = name_str.parse::<i32>() else {
                continue;
            };
            if pid <= 0 {
                continue;
            }

            path_buf.clear();
            path_buf.push(proc_root);
            path_buf.push(name_str);

            path_buf.push("stat");
            let stat_ok = match read_into_buf(&path_buf, &mut scratch) {
                Ok(()) => true,
                Err(err) if is_vanished_process_error(&err) => false,
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => false,
                Err(err) => return Err(err.into()),
            };
            path_buf.pop();
            if !stat_ok {
                continue;
            }

            let Ok(stat_content) = std::str::from_utf8(&scratch) else {
                continue;
            };
            let Ok(stat) = parse_proc_pid_stat(stat_content) else {
                continue;
            };

            path_buf.push("status");
            let uid = match read_into_buf(&path_buf, &mut scratch) {
                Ok(()) => std::str::from_utf8(&scratch)
                    .ok()
                    .and_then(parse_proc_pid_status_uid)
                    .unwrap_or(0),
                Err(err) if is_vanished_process_error(&err) => {
                    path_buf.pop();
                    continue;
                }
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => 0,
                Err(err) => return Err(err.into()),
            };
            path_buf.pop();

            let user = self
                .uid_cache
                .get(&uid)
                .cloned()
                .unwrap_or_else(|| uid.to_string());

            path_buf.push("cmdline");
            let cmdline = match read_into_buf(&path_buf, &mut scratch) {
                Ok(()) => format_cmdline(&scratch, &stat.comm),
                Err(err) if is_vanished_process_error(&err) => {
                    path_buf.pop();
                    continue;
                }
                Err(_) => format!("[{}]", stat.comm),
            };
            path_buf.pop();

            let curr_ticks = stat.utime.saturating_add(stat.stime);
            next_ticks.insert(pid, curr_ticks);

            let cpu_pct = match self.prev_ticks.get(&pid) {
                Some(&prev) if total_cpu_delta_ticks > 0 => {
                    let delta = curr_ticks.saturating_sub(prev);
                    (delta as f64 / total_cpu_delta_ticks as f64) * 100.0 * core_scale
                }
                _ => 0.0,
            };

            let rss_bytes = stat.rss_pages.saturating_mul(self.page_size);
            let mem_pct = if total_mem_bytes > 0 {
                (rss_bytes as f64 / total_mem_bytes as f64) * 100.0
            } else {
                0.0
            };

            processes.push(ProcessInfo {
                pid: stat.pid,
                ppid: stat.ppid,
                user,
                state: stat.state,
                priority: stat.priority,
                nice: stat.nice,
                rss_bytes,
                vsize_bytes: stat.vsize,
                mem_pct,
                cpu_pct,
                cpu_time_ticks: curr_ticks,
                comm: stat.comm,
                cmdline,
            });
        }

        self.prev_ticks = next_ticks;
        Ok(processes)
    }
}

fn read_into_buf(path: &Path, buf: &mut Vec<u8>) -> io::Result<()> {
    buf.clear();
    let mut file = File::open(path)?;
    file.read_to_end(buf)?;
    Ok(())
}

fn query_sysconf(name: libc::c_int, fallback: u64) -> u64 {
    // SAFETY: `sysconf` is thread-safe for static system constants `_SC_PAGESIZE` and `_SC_CLK_TCK`.
    let val = unsafe { libc::sysconf(name) };
    if val > 0 { val as u64 } else { fallback }
}

fn sanitize_control_chars(s: &str) -> String {
    s.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn is_vanished_process_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ESRCH)
}

/// Parses `/proc/[pid]/stat` handling process names (`comm`) containing spaces or `)`.
pub fn parse_proc_pid_stat(content: &str) -> Result<ProcStatRaw, CollectorError> {
    let open_paren = content
        .find('(')
        .ok_or_else(|| CollectorError::Parse("missing '(' in /proc/[pid]/stat".into()))?;
    let close_paren = content
        .rfind(')')
        .ok_or_else(|| CollectorError::Parse("missing ')' in /proc/[pid]/stat".into()))?;

    if close_paren <= open_paren {
        return Err(CollectorError::Parse(
            "malformed '(comm)' in /proc/[pid]/stat".into(),
        ));
    }

    let pid_str = content[..open_paren].trim();
    let pid: i32 = pid_str
        .parse()
        .map_err(|_| CollectorError::Parse(format!("invalid pid '{pid_str}'")))?;

    let comm = sanitize_control_chars(&content[open_paren + 1..close_paren]);
    let rest = &content[close_paren + 1..];

    let mut f_state = None;
    let mut f_ppid = None;
    let mut f_utime = None;
    let mut f_stime = None;
    let mut f_priority = None;
    let mut f_nice = None;
    let mut f_num_threads = None;
    let mut f_vsize = None;
    let mut f_rss = None;
    let mut field_count = 0usize;

    for (idx, token) in rest.split_ascii_whitespace().enumerate() {
        field_count = idx + 1;
        match idx {
            0 => f_state = Some(token),
            1 => f_ppid = Some(token),
            11 => f_utime = Some(token),
            12 => f_stime = Some(token),
            15 => f_priority = Some(token),
            16 => f_nice = Some(token),
            17 => f_num_threads = Some(token),
            20 => f_vsize = Some(token),
            21 => f_rss = Some(token),
            _ => {}
        }
    }

    // After `(comm)`, field 0 is `(3) state` and field 21 is `(24) rss`.
    if field_count < 22 {
        return Err(CollectorError::Parse(format!(
            "expected at least 22 fields after comm, got {field_count}"
        )));
    }

    let state_char = f_state
        .and_then(|s| s.chars().next())
        .ok_or_else(|| CollectorError::Parse("empty state field".into()))?;
    let state = ProcessState::from_char(state_char);

    let ppid = parse_field::<i32>(f_ppid.unwrap_or_default(), "ppid")?;
    let utime = parse_field::<u64>(f_utime.unwrap_or_default(), "utime")?;
    let stime = parse_field::<u64>(f_stime.unwrap_or_default(), "stime")?;
    let priority = parse_field::<i64>(f_priority.unwrap_or_default(), "priority")?;
    let nice = parse_field::<i64>(f_nice.unwrap_or_default(), "nice")?;
    let num_threads = parse_field::<i64>(f_num_threads.unwrap_or_default(), "num_threads")?;
    let vsize = parse_field::<u64>(f_vsize.unwrap_or_default(), "vsize")?;
    let rss_raw = parse_field::<i64>(f_rss.unwrap_or_default(), "rss")?;
    let rss_pages = u64::try_from(rss_raw.max(0)).unwrap_or(0);

    Ok(ProcStatRaw {
        pid,
        comm,
        state,
        ppid,
        utime,
        stime,
        priority,
        nice,
        num_threads,
        vsize,
        rss_pages,
    })
}

fn parse_field<T: std::str::FromStr>(raw: &str, name: &str) -> Result<T, CollectorError> {
    raw.parse::<T>()
        .map_err(|_| CollectorError::Parse(format!("invalid {name} '{raw}'")))
}

/// Parses the real UID from `/proc/[pid]/status` (`Uid:\t<real> <effective> ...`).
pub fn parse_proc_pid_status_uid(content: &str) -> Option<u32> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_ascii_whitespace().next()?.parse::<u32>().ok();
        }
    }
    None
}

/// Parses `/etc/passwd` into a `UID -> username` map.
pub fn parse_passwd(content: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split(':');
        let Some(name) = parts.next() else {
            continue;
        };
        let _password = parts.next();
        let Some(uid_str) = parts.next() else {
            continue;
        };
        if let Ok(uid) = uid_str.parse::<u32>() {
            map.entry(uid)
                .or_insert_with(|| sanitize_control_chars(name));
        }
    }
    map
}

/// Formats `/proc/[pid]/cmdline` bytes by replacing `\0` and control characters with spaces
/// and falling back to `[comm]` when empty (e.g., kernel threads).
pub fn format_cmdline(raw_bytes: &[u8], comm: &str) -> String {
    let is_sep = |b: u8| b == 0 || b.is_ascii_whitespace();
    let start = raw_bytes.iter().position(|&b| !is_sep(b));
    let Some(start) = start else {
        return format!("[{}]", sanitize_control_chars(comm));
    };
    let end = raw_bytes
        .iter()
        .rposition(|&b| !is_sep(b))
        .map_or(start, |idx| idx + 1);
    let trimmed_bytes = &raw_bytes[start..end];

    let lossy = String::from_utf8_lossy(trimmed_bytes);
    let mut out = String::with_capacity(lossy.len());
    for ch in lossy.chars() {
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    out
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
            "rstop_proc_test_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_proc_pid(proc_root: &Path, pid: i32, stat: &str, status: &str, cmdline: &[u8]) {
        let pid_dir = proc_root.join(pid.to_string());
        fs::create_dir_all(&pid_dir).expect("create pid dir");
        fs::write(pid_dir.join("stat"), stat).expect("write stat");
        fs::write(pid_dir.join("status"), status).expect("write status");
        fs::write(pid_dir.join("cmdline"), cmdline).expect("write cmdline");
    }

    #[test]
    fn parse_proc_pid_stat_handles_spaces_and_parentheses_in_comm() {
        let line = "4210 (Web Content (1) test) R 1000 4210 1000 0 -1 4194304 123 0 1 0 250 50 0 0 20 0 4 0 98765 10485760 256 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n";
        let parsed = parse_proc_pid_stat(line).expect("should parse stat");
        assert_eq!(parsed.pid, 4210);
        assert_eq!(parsed.comm, "Web Content (1) test");
        assert_eq!(parsed.state, ProcessState::Running);
        assert_eq!(parsed.ppid, 1000);
        assert_eq!(parsed.utime, 250);
        assert_eq!(parsed.stime, 50);
        assert_eq!(parsed.priority, 20);
        assert_eq!(parsed.nice, 0);
        assert_eq!(parsed.num_threads, 4);
        assert_eq!(parsed.vsize, 10_485_760);
        assert_eq!(parsed.rss_pages, 256);
    }

    #[test]
    fn parse_proc_pid_stat_rejects_malformed() {
        assert!(parse_proc_pid_stat("123 no_parens S 1").is_err());
        assert!(parse_proc_pid_stat("123 (short) S 1 2 3").is_err());
    }

    #[test]
    fn format_cmdline_replaces_nulls_and_falls_back_to_comm() {
        assert_eq!(
            format_cmdline(b"/usr/bin/python3\0-m\0http.server\08000\0", "python3"),
            "/usr/bin/python3 -m http.server 8000"
        );
        assert_eq!(format_cmdline(b"", "kthreadd"), "[kthreadd]");
        assert_eq!(format_cmdline(b"\0\0\0", "kworker/0:1"), "[kworker/0:1]");
    }

    #[test]
    fn parse_proc_pid_status_uid_extracts_real_uid() {
        let status = "\
Name:\tbash
Umask:\t0022
State:\tS (sleeping)
Tgid:\t1234
Pid:\t1234
Uid:\t1000\t1000\t1000\t1000
Gid:\t1000\t1000\t1000\t1000
";
        assert_eq!(parse_proc_pid_status_uid(status), Some(1000));
        assert_eq!(parse_proc_pid_status_uid("Name:\tinit\n"), None);
    }

    #[test]
    fn parse_passwd_maps_uids_and_ignores_comments() {
        let passwd = "\
# comment line
root:x:0:0:root:/root:/bin/bash
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin

awaiden:x:1000:1000:Awaiden:/home/awaiden:/bin/zsh
badline
";
        let map = parse_passwd(passwd);
        assert_eq!(map.get(&0).map(String::as_str), Some("root"));
        assert_eq!(map.get(&1).map(String::as_str), Some("daemon"));
        assert_eq!(map.get(&1000).map(String::as_str), Some("awaiden"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn process_state_roundtrip() {
        for (ch, expected) in [
            ('R', ProcessState::Running),
            ('S', ProcessState::Sleeping),
            ('D', ProcessState::DiskSleep),
            ('Z', ProcessState::Zombie),
            ('T', ProcessState::Stopped),
            ('t', ProcessState::TracingStop),
            ('I', ProcessState::Idle),
            ('X', ProcessState::Unknown('X')),
        ] {
            let state = ProcessState::from_char(ch);
            assert_eq!(state, expected);
            assert_eq!(state.as_char(), ch);
        }
    }

    #[test]
    fn collect_from_proc_computes_cpu_delta_and_mem_pct() {
        let proc_root = make_temp_proc_dir("delta");
        let passwd = parse_passwd(
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000::/home/alice:/bin/sh\n",
        );
        let mut collector = ProcessCollector::with_config(passwd, 4096, 100);

        // Tick 1: pid 100 has utime=100, stime=50 (total=150), rss=256 pages (1 MiB)
        let stat1 = "100 (worker) S 1 100 100 0 -1 0 0 0 0 0 100 50 0 0 20 0 2 0 100 2097152 256 0";
        write_proc_pid(
            &proc_root,
            100,
            stat1,
            "Uid:\t1000\t1000\t1000\t1000\n",
            b"/bin/worker\0--fast\0",
        );
        // Also a non-numeric entry that must be ignored
        fs::create_dir_all(proc_root.join("self")).expect("create self");

        let snap1 = collector
            .collect_from_proc(&proc_root, 0, 1024 * 1024, 4)
            .expect("first collect");
        assert_eq!(snap1.len(), 1);
        assert_eq!(snap1[0].cpu_pct, 0.0);
        assert_eq!(snap1[0].user, "alice");
        assert_eq!(snap1[0].cmdline, "/bin/worker --fast");
        assert_eq!(snap1[0].rss_bytes, 256 * 4096);
        // 1024 KiB / 1048576 KiB = 1/1024 = 0.09765625%
        assert!((snap1[0].mem_pct - (1024.0 / (1024.0 * 1024.0) * 100.0)).abs() < 1e-9);

        // Tick 2: pid 100 advances by +50 ticks (utime=130, stime=70 => total=200)
        // Total CPU delta across 4 cores = 400 ticks => 50/400 * 100 * 4 = 50.0%
        let stat2 = "100 (worker) R 1 100 100 0 -1 0 0 0 0 0 130 70 0 0 20 0 2 0 100 2097152 256 0";
        write_proc_pid(
            &proc_root,
            100,
            stat2,
            "Uid:\t1000\t1000\t1000\t1000\n",
            b"/bin/worker\0--fast\0",
        );

        // Also pid 200 with unknown UID 9999 and empty cmdline
        let stat_k = "200 (kworker) I 2 0 0 0 -1 0 0 0 0 0 10 0 0 0 20 -20 1 0 50 0 0 0";
        write_proc_pid(
            &proc_root,
            200,
            stat_k,
            "Uid:\t9999\t9999\t9999\t9999\n",
            b"",
        );

        let mut snap2 = collector
            .collect_from_proc(&proc_root, 400, 1024 * 1024, 4)
            .expect("second collect");
        snap2.sort_by_key(|p| p.pid);

        assert_eq!(snap2.len(), 2);
        assert_eq!(snap2[0].pid, 100);
        assert!((snap2[0].cpu_pct - 50.0).abs() < 1e-9);
        assert_eq!(snap2[0].cpu_time_ticks, 200);
        assert_eq!(snap2[0].state, ProcessState::Running);

        assert_eq!(snap2[1].pid, 200);
        assert_eq!(snap2[1].user, "9999");
        assert_eq!(snap2[1].cmdline, "[kworker]");
        assert_eq!(snap2[1].cpu_pct, 0.0);

        let _ = fs::remove_dir_all(&proc_root);
    }

    #[test]
    fn collect_from_proc_ignores_vanished_process_directory() {
        let proc_root = make_temp_proc_dir("vanished");
        let mut collector = ProcessCollector::with_config(HashMap::new(), 4096, 100);

        // Create a PID directory that is missing `stat` (simulates process exiting mid-scan).
        fs::create_dir_all(proc_root.join("99999")).expect("create empty pid dir");

        let stat_ok = "101 (init) S 0 1 1 0 -1 0 0 0 0 0 10 5 0 0 20 0 1 0 1 1024 10 0";
        write_proc_pid(
            &proc_root,
            101,
            stat_ok,
            "Uid:\t0\t0\t0\t0\n",
            b"/sbin/init\0",
        );

        let procs = collector
            .collect_from_proc(&proc_root, 100, 1024, 1)
            .expect("vanished pid should not fail collection");
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].pid, 101);

        let _ = fs::remove_dir_all(&proc_root);
    }

    #[test]
    fn untrusted_strings_strip_control_and_escape_sequences() {
        // comm with ANSI CSI + OSC (\x1b, \x07, \r, \n)
        let stat_line = "555 (evil\x1b[31m\x1b]0;pwn\x07\r\n) S 1 555 555 0 -1 0 0 0 0 0 1 0 0 0 20 0 1 0 10 1024 1 0";
        let parsed = parse_proc_pid_stat(stat_line).expect("should parse stat");
        assert!(!parsed.comm.chars().any(char::is_control));
        assert_eq!(parsed.comm, "evil [31m ]0;pwn   ");

        // cmdline with ANSI/OSC/newline/carriage return
        let cmd = format_cmdline(b"/bin/app\0--title=\x1b]2;hacked\x07\r\n--ok", "app");
        assert!(!cmd.chars().any(char::is_control));
        assert_eq!(cmd, "/bin/app --title= ]2;hacked   --ok");

        // empty cmdline fallback also sanitizes comm
        let fallback = format_cmdline(b"", "kworker\x1b[2J\x07");
        assert!(!fallback.chars().any(char::is_control));
        assert_eq!(fallback, "[kworker [2J ]");

        // /etc/passwd username with control characters
        let passwd = parse_passwd("bad\x1b[31m\x07user:x:1234:1234::/home/u:/bin/sh\n");
        let name = passwd.get(&1234).expect("uid 1234");
        assert!(!name.chars().any(char::is_control));
        assert_eq!(name, "bad [31m user");
    }
}
