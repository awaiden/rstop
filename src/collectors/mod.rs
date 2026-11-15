pub mod cpu;
pub mod memory;
pub mod network;
pub mod process;

pub use process::{ProcessInfo, ProcessState};

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Error returned when reading or parsing `/proc` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorError {
    Io(String),
    Parse(String),
}

impl fmt::Display for CollectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Parse(msg) => write!(f, "Parse error: {msg}"),
        }
    }
}

impl std::error::Error for CollectorError {}

impl From<std::io::Error> for CollectorError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Bounded ring buffer backed by `VecDeque<T>` for metric history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryBuffer<T> {
    buf: VecDeque<T>,
    capacity: usize,
}

impl<T> HistoryBuffer<T> {
    /// Creates a new `HistoryBuffer` retaining at most `capacity` elements.
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Appends a value, evicting the oldest element if at capacity.
    pub fn push(&mut self, value: T) {
        if self.capacity == 0 {
            return;
        }
        if self.buf.len() == self.capacity {
            self.buf.pop_front();
        }
        self.buf.push_back(value);
    }

    /// Returns an iterator from oldest to newest element.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.buf.iter()
    }
}

/// Raw CPU jiffies counters from `/proc/stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
    pub guest: u64,
    pub guest_nice: u64,
}

impl CpuTimes {
    /// Total CPU jiffies across fields `0..8`, excluding `guest` and `guest_nice`
    /// because Linux already accounts them inside `user` and `nice`.
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    /// Combined idle and iowait jiffies.
    pub fn idle_all(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }
}

/// Percentage breakdown of CPU usage over a sampling interval.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuUsage {
    pub total_pct: f64,
    pub user_pct: f64,
    pub sys_pct: f64,
    pub iowait_pct: f64,
}

/// Aggregate and per-core CPU usage plus total usage history.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuStats {
    pub total: CpuUsage,
    pub cores: Vec<CpuUsage>,
    pub history: HistoryBuffer<f64>,
}

/// Memory and swap statistics in KiB from `/proc/meminfo`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemStats {
    pub total_kb: u64,
    pub used_kb: u64,
    pub free_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_used_kb: u64,
}

/// Load averages, system uptime, and thread counts from `/proc/loadavg` and `/proc/uptime`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoadUptime {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub uptime_secs: f64,
    pub running_threads: u32,
    pub total_threads: u32,
}

/// Network interface computed transfer rates.
#[derive(Debug, Clone, PartialEq)]
pub struct NetInterfaceStats {
    pub name: String,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

/// Combined system metrics snapshot for a single collection tick.
#[derive(Debug, Clone)]
pub struct SystemSnapshot {
    pub cpu: CpuStats,
    pub mem: MemStats,
    pub load: LoadUptime,
    pub net: Vec<NetInterfaceStats>,
    pub processes: Vec<ProcessInfo>,
    pub total_cpu_delta_ticks: u64,
    pub clock_ticks_per_sec: u64,
}

/// Stateful collector that samples `/proc` and computes deltas across ticks.
#[derive(Debug)]
pub struct SystemCollector {
    proc_root: PathBuf,
    prev_total_cpu: Option<CpuTimes>,
    prev_core_cpus: Vec<CpuTimes>,
    prev_net: Vec<network::NetRaw>,
    prev_collected_at: Option<Instant>,
    cpu_history: HistoryBuffer<f64>,
    process_collector: process::ProcessCollector,
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new(120)
    }
}

impl SystemCollector {
    /// Creates a collector reading from `/proc` with the given CPU history capacity.
    pub fn new(history_capacity: usize) -> Self {
        Self::with_proc_root(PathBuf::from("/proc"), history_capacity)
    }

    /// Creates a collector reading from a custom procfs root directory (useful for tests).
    pub fn with_proc_root(proc_root: PathBuf, history_capacity: usize) -> Self {
        Self {
            proc_root,
            prev_total_cpu: None,
            prev_core_cpus: Vec::new(),
            prev_net: Vec::new(),
            prev_collected_at: None,
            cpu_history: HistoryBuffer::new(history_capacity),
            process_collector: process::ProcessCollector::new(),
        }
    }

    /// Collects a full `SystemSnapshot` from `proc_root`.
    pub fn collect(&mut self) -> Result<SystemSnapshot, CollectorError> {
        let now = Instant::now();

        let stat_content = read_proc_file(&self.proc_root, "stat")?;
        let (total_times, core_times) = cpu::parse_proc_stat(&stat_content)?;

        let (total_usage, total_cpu_delta_ticks) = match self.prev_total_cpu {
            Some(prev) => cpu::compute_cpu_delta(&prev, &total_times),
            None => (CpuUsage::default(), 0),
        };

        let cores_usage: Vec<CpuUsage> = core_times
            .iter()
            .enumerate()
            .map(|(idx, curr)| match self.prev_core_cpus.get(idx) {
                Some(prev) => cpu::compute_cpu_delta(prev, curr).0,
                None => CpuUsage::default(),
            })
            .collect();

        if self.prev_total_cpu.is_some() {
            self.cpu_history.push(total_usage.total_pct);
        }

        let mem_content = read_proc_file(&self.proc_root, "meminfo")?;
        let mem = memory::parse_meminfo(&mem_content)?;

        let loadavg_content = read_proc_file(&self.proc_root, "loadavg")?;
        let (load1, load5, load15, running_threads, total_threads) =
            cpu::parse_loadavg(&loadavg_content)?;

        let uptime_content = read_proc_file(&self.proc_root, "uptime")?;
        let uptime_secs = cpu::parse_uptime(&uptime_content)?;

        let net_content = read_proc_file(&self.proc_root, "net/dev")?;
        let curr_net = network::parse_net_dev(&net_content)?;
        let elapsed_secs = self
            .prev_collected_at
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64())
            .unwrap_or(0.0);
        let net = network::compute_net_rates(&self.prev_net, &curr_net, elapsed_secs);

        self.prev_total_cpu = Some(total_times);
        self.prev_core_cpus = core_times;
        self.prev_net = curr_net;
        self.prev_collected_at = Some(now);

        let num_cores = cores_usage.len();
        let processes = self.process_collector.collect_from_proc(
            &self.proc_root,
            total_cpu_delta_ticks,
            mem.total_kb,
            num_cores,
        )?;

        Ok(SystemSnapshot {
            cpu: CpuStats {
                total: total_usage,
                cores: cores_usage,
                history: self.cpu_history.clone(),
            },
            mem,
            load: LoadUptime {
                load1,
                load5,
                load15,
                uptime_secs,
                running_threads,
                total_threads,
            },
            net,
            processes,
            total_cpu_delta_ticks,
            clock_ticks_per_sec: self.process_collector.clock_ticks_per_sec(),
        })
    }
}

fn read_proc_file(proc_root: &Path, relative: &str) -> Result<String, CollectorError> {
    let path = proc_root.join(relative);
    fs::read_to_string(&path)
        .map_err(|err| CollectorError::Io(format!("failed to read {}: {err}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn history_buffer_evicts_oldest_at_capacity() {
        let mut buf = HistoryBuffer::new(3);
        assert_eq!(buf.iter().count(), 0);

        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        assert_eq!(buf.iter().copied().collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);

        buf.push(4.0);
        assert_eq!(buf.iter().copied().collect::<Vec<_>>(), vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn history_buffer_zero_capacity_is_noop() {
        let mut buf = HistoryBuffer::new(0);
        buf.push(42.0);
        assert_eq!(buf.iter().count(), 0);
    }

    fn make_temp_proc_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rstop_sys_test_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_proc_fixture(
        proc_root: &Path,
        cpu_user: u64,
        cpu_idle: u64,
        rx_bytes: u64,
        tx_bytes: u64,
        proc_utime: u64,
    ) {
        fs::create_dir_all(proc_root.join("net")).expect("create net dir");
        fs::create_dir_all(proc_root.join("101")).expect("create pid dir");

        let stat = format!(
            "cpu  {cpu_user} 0 0 {cpu_idle} 0 0 0 0 0 0\n\
             cpu0 {cpu_user} 0 0 {cpu_idle} 0 0 0 0 0 0\n"
        );
        fs::write(proc_root.join("stat"), stat).expect("write stat");

        let meminfo = "\
MemTotal:       1000000 kB
MemFree:         200000 kB
Buffers:          50000 kB
Cached:          100000 kB
SwapTotal:       500000 kB
SwapFree:        400000 kB
";
        fs::write(proc_root.join("meminfo"), meminfo).expect("write meminfo");
        fs::write(proc_root.join("loadavg"), "0.50 0.25 0.10 1/100 101\n").expect("write loadavg");
        fs::write(proc_root.join("uptime"), "1234.56 2000.00\n").expect("write uptime");

        let net_dev = format!(
            "Inter-|   Receive                                                |  Transmit\n\
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
             eth0: {rx_bytes} 10 0 0 0 0 0 0 {tx_bytes} 10 0 0 0 0 0 0\n"
        );
        fs::write(proc_root.join("net/dev"), net_dev).expect("write net/dev");

        let proc_stat = format!(
            "101 (worker) R 1 101 101 0 -1 4194304 0 0 0 0 {proc_utime} 0 0 0 20 0 1 0 100 1048576 256 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n"
        );
        fs::write(proc_root.join("101/stat"), proc_stat).expect("write pid stat");
        fs::write(proc_root.join("101/status"), "Uid:\t0\t0\t0\t0\n").expect("write pid status");
        fs::write(proc_root.join("101/cmdline"), b"/usr/bin/worker\0").expect("write pid cmdline");
    }

    #[test]
    fn system_collector_multi_tick_computes_history_net_rates_and_process_deltas() {
        let proc_root = make_temp_proc_dir("multi_tick");
        let mut collector = SystemCollector::with_proc_root(proc_root.clone(), 10);

        // Tick 1: baseline
        write_proc_fixture(&proc_root, 100, 900, 1000, 500, 10);
        let snap1 = collector.collect().expect("tick 1");
        assert_eq!(snap1.cpu.history.iter().count(), 0);
        assert_eq!(snap1.net.len(), 1);
        assert_eq!(snap1.net[0].rx_bytes_per_sec, 0.0);
        assert_eq!(snap1.net[0].tx_bytes_per_sec, 0.0);
        assert_eq!(snap1.processes.len(), 1);
        assert_eq!(snap1.processes[0].cpu_pct, 0.0);

        std::thread::sleep(Duration::from_millis(20));

        // Tick 2: +50 user, +50 idle (total delta = 100, 50% CPU), +1000 rx, +500 tx, +25 proc utime
        write_proc_fixture(&proc_root, 150, 950, 2000, 1000, 35);
        let snap2 = collector.collect().expect("tick 2");
        let history: Vec<f64> = snap2.cpu.history.iter().copied().collect();
        assert_eq!(history.len(), 1);
        assert!((history[0] - 50.0).abs() < 1e-6);
        assert!((snap2.cpu.total.total_pct - 50.0).abs() < 1e-6);

        assert_eq!(snap2.net.len(), 1);
        assert!(snap2.net[0].rx_bytes_per_sec > 0.0);
        assert!(snap2.net[0].tx_bytes_per_sec > 0.0);

        assert_eq!(snap2.processes.len(), 1);
        // 25 / 100 * 100.0 * 1 core = 25.0%
        assert!((snap2.processes[0].cpu_pct - 25.0).abs() < 1e-6);
        assert_eq!(snap2.processes[0].cpu_time_ticks, 35);

        let _ = fs::remove_dir_all(&proc_root);
    }
}
