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

    /// Returns the underlying contiguous slices of the ring buffer.
    pub fn as_slices(&self) -> (&[T], &[T]) {
        self.buf.as_slices()
    }

    /// Returns an iterator from oldest to newest element.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.buf.iter()
    }

    /// Returns the most recently pushed element, if any.
    pub fn latest(&self) -> Option<&T> {
        self.buf.back()
    }

    /// Returns the maximum number of elements retained by this buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of stored elements.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns `true` if the buffer contains no elements.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
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
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub shared_kb: u64,
    pub swap_total_kb: u64,
    pub swap_used_kb: u64,
    pub swap_cached_kb: u64,
    pub swap_free_kb: u64,
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

/// Network interface counters and computed transfer rates.
#[derive(Debug, Clone, PartialEq)]
pub struct NetInterfaceStats {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
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
    pub num_cores: usize,
    pub collected_at: Instant,
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
            num_cores,
            collected_at: now,
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

    #[test]
    fn history_buffer_evicts_oldest_at_capacity() {
        let mut buf = HistoryBuffer::new(3);
        assert_eq!(buf.capacity(), 3);
        assert!(buf.is_empty());
        assert_eq!(buf.latest(), None);

        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.latest(), Some(&3.0));

        buf.push(4.0);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.latest(), Some(&4.0));
        let collected: Vec<f64> = buf.iter().copied().collect();
        assert_eq!(collected, vec![2.0, 3.0, 4.0]);

        let (a, b) = buf.as_slices();
        let combined: Vec<f64> = a.iter().chain(b.iter()).copied().collect();
        assert_eq!(combined, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn history_buffer_zero_capacity_is_noop() {
        let mut buf = HistoryBuffer::new(0);
        buf.push(42.0);
        assert!(buf.is_empty());
        assert_eq!(buf.latest(), None);
        assert_eq!(buf.capacity(), 0);
    }
}
