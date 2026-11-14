use super::{CollectorError, CpuTimes, CpuUsage};

/// Parses `/proc/stat` content into `(aggregate_cpu, per_core_cpus)`.
///
/// Handles 4 to 10+ numeric fields per `cpu` line:
/// `user nice system idle iowait irq softirq steal guest guest_nice`.
pub fn parse_proc_stat(content: &str) -> Result<(CpuTimes, Vec<CpuTimes>), CollectorError> {
    let mut aggregate: Option<CpuTimes> = None;
    let mut cores: Vec<CpuTimes> = Vec::new();

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(label) = parts.next() else {
            continue;
        };

        if label == "cpu" {
            aggregate = Some(parse_cpu_fields(parts)?);
        } else if let Some(suffix) = label.strip_prefix("cpu")
            && !suffix.is_empty()
            && suffix.bytes().all(|b| b.is_ascii_digit())
        {
            cores.push(parse_cpu_fields(parts)?);
        }
    }

    let total = aggregate.ok_or_else(|| {
        CollectorError::Parse("missing aggregate 'cpu' line in /proc/stat".into())
    })?;
    Ok((total, cores))
}

fn parse_cpu_fields<'a>(fields: impl Iterator<Item = &'a str>) -> Result<CpuTimes, CollectorError> {
    let mut values = [0u64; 10];
    let mut count = 0usize;

    for token in fields {
        let val = token
            .parse::<u64>()
            .map_err(|e| CollectorError::Parse(format!("invalid cpu jiffies '{token}': {e}")))?;
        if count < values.len() {
            values[count] = val;
        }
        count += 1;
    }

    if count < 4 {
        return Err(CollectorError::Parse(format!(
            "expected at least 4 cpu fields in /proc/stat, found {count}"
        )));
    }

    Ok(CpuTimes {
        user: values[0],
        nice: values[1],
        system: values[2],
        idle: values[3],
        iowait: values[4],
        irq: values[5],
        softirq: values[6],
        steal: values[7],
        guest: values[8],
        guest_nice: values[9],
    })
}

/// Computes CPU usage percentages and total delta ticks between two `CpuTimes` samples.
///
/// `guest` and `guest_nice` are excluded from `CpuTimes::total()` since Linux already
/// accounts them inside `user` and `nice`. All returned percentages are clamped to `[0.0, 100.0]`.
pub fn compute_cpu_delta(prev: &CpuTimes, curr: &CpuTimes) -> (CpuUsage, u64) {
    let delta_total = curr.total().saturating_sub(prev.total());
    if delta_total == 0 {
        return (CpuUsage::default(), 0);
    }

    let delta_idle = curr.idle_all().saturating_sub(prev.idle_all());
    let delta_active = delta_total.saturating_sub(delta_idle);

    let delta_user = curr
        .user
        .saturating_sub(prev.user)
        .saturating_add(curr.nice.saturating_sub(prev.nice));
    let delta_sys = curr
        .system
        .saturating_sub(prev.system)
        .saturating_add(curr.irq.saturating_sub(prev.irq))
        .saturating_add(curr.softirq.saturating_sub(prev.softirq));
    let delta_iowait = curr.iowait.saturating_sub(prev.iowait);

    let denom = delta_total as f64;
    let to_pct = |ticks: u64| ((ticks as f64 / denom) * 100.0).clamp(0.0, 100.0);

    let usage = CpuUsage {
        total_pct: to_pct(delta_active),
        user_pct: to_pct(delta_user),
        sys_pct: to_pct(delta_sys),
        iowait_pct: to_pct(delta_iowait),
    };

    (usage, delta_total)
}

/// Parses `/proc/loadavg` into `(load1, load5, load15, running_threads, total_threads)`.
pub fn parse_loadavg(content: &str) -> Result<(f64, f64, f64, u32, u32), CollectorError> {
    let mut parts = content.split_whitespace();
    let l1 = parse_f64_token(parts.next(), "load1")?;
    let l5 = parse_f64_token(parts.next(), "load5")?;
    let l15 = parse_f64_token(parts.next(), "load15")?;

    let threads_tok = parts
        .next()
        .ok_or_else(|| CollectorError::Parse("missing threads field in /proc/loadavg".into()))?;
    let (run_str, total_str) = threads_tok.split_once('/').ok_or_else(|| {
        CollectorError::Parse(format!(
            "malformed threads field '{threads_tok}' in /proc/loadavg"
        ))
    })?;

    let running_threads = run_str
        .parse::<u32>()
        .map_err(|e| CollectorError::Parse(format!("invalid running threads '{run_str}': {e}")))?;
    let total_threads = total_str
        .parse::<u32>()
        .map_err(|e| CollectorError::Parse(format!("invalid total threads '{total_str}': {e}")))?;

    Ok((l1, l5, l15, running_threads, total_threads))
}

/// Parses `/proc/uptime` and returns system uptime in seconds.
pub fn parse_uptime(content: &str) -> Result<f64, CollectorError> {
    let token = content.split_whitespace().next();
    let secs = parse_f64_token(token, "uptime")?;
    if secs < 0.0 {
        return Err(CollectorError::Parse(format!(
            "negative uptime in /proc/uptime: {secs}"
        )));
    }
    Ok(secs)
}

fn parse_f64_token(token: Option<&str>, field: &str) -> Result<f64, CollectorError> {
    let raw = token.ok_or_else(|| CollectorError::Parse(format!("missing {field} field")))?;
    let val = raw
        .parse::<f64>()
        .map_err(|e| CollectorError::Parse(format!("invalid {field} '{raw}': {e}")))?;
    if !val.is_finite() {
        return Err(CollectorError::Parse(format!(
            "non-finite {field} value '{raw}'"
        )));
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_STAT: &str = "\
cpu  10132153 290696 3084719 46828483 16683 0 25195 0 1234 567
cpu0 2533038 72674 771179 11707120 4170 0 6298 0 300 140
cpu1 2533039 72674 771180 11707121 4171 0 6299 0 310 141
cpu2 2533038 72674 771180 11707121 4171 0 6299 0 312 143
cpu3 2533038 72674 771180 11707121 4171 0 6299 0 312 143
intr 123456 0 0
ctxt 987654321
btime 1700000000
processes 45678
procs_running 2
procs_blocked 0
";

    #[test]
    fn parse_proc_stat_parses_aggregate_and_cores() {
        let (total, cores) = parse_proc_stat(SAMPLE_STAT).expect("valid /proc/stat");
        assert_eq!(cores.len(), 4);
        assert_eq!(total.user, 10_132_153);
        assert_eq!(total.nice, 290_696);
        assert_eq!(total.system, 3_084_719);
        assert_eq!(total.idle, 46_828_483);
        assert_eq!(total.iowait, 16_683);
        assert_eq!(total.softirq, 25_195);
        assert_eq!(total.guest, 1234);
        assert_eq!(total.guest_nice, 567);

        // Verify guest and guest_nice are excluded from total()
        let expected_total = 10_132_153 + 290_696 + 3_084_719 + 46_828_483 + 16_683 + 25_195;
        assert_eq!(total.total(), expected_total);
    }

    #[test]
    fn parse_proc_stat_handles_four_fields_and_extra_fields() {
        let stat = "\
cpu  100 20 30 400
cpu0 50 10 15 200 5 1 2 3 99 88 77 66
";
        let (total, cores) = parse_proc_stat(stat).expect("valid minimal/extended /proc/stat");
        assert_eq!(total.total(), 550);
        assert_eq!(total.iowait, 0);
        assert_eq!(cores.len(), 1);
        assert_eq!(cores[0].guest, 99);
        assert_eq!(cores[0].guest_nice, 88);
        assert_eq!(cores[0].total(), 50 + 10 + 15 + 200 + 5 + 1 + 2 + 3);
    }

    #[test]
    fn parse_proc_stat_rejects_empty_or_malformed() {
        assert!(parse_proc_stat("").is_err());
        assert!(parse_proc_stat("intr 1 2 3\n").is_err());
        assert!(parse_proc_stat("cpu 10 20 30\n").is_err());
        assert!(parse_proc_stat("cpu 10 abc 30 40\n").is_err());
    }

    #[test]
    fn compute_cpu_delta_excludes_guest_and_computes_percentages() {
        let prev = CpuTimes {
            user: 1000,
            nice: 100,
            system: 200,
            idle: 8000,
            iowait: 100,
            irq: 0,
            softirq: 0,
            steal: 0,
            guest: 500,
            guest_nice: 50,
        };
        let curr = CpuTimes {
            user: 1200,  // +200
            nice: 100,   // +0
            system: 250, // +50
            idle: 8700,  // +700
            iowait: 150, // +50
            irq: 0,
            softirq: 0,
            steal: 0,
            guest: 600,      // +100 (must NOT be added to total_delta)
            guest_nice: 100, // +50  (must NOT be added to total_delta)
        };

        let (usage, delta_ticks) = compute_cpu_delta(&prev, &curr);
        assert_eq!(delta_ticks, 1000);
        assert!((usage.total_pct - 25.0).abs() < 1e-6);
        assert!((usage.user_pct - 20.0).abs() < 1e-6);
        assert!((usage.sys_pct - 5.0).abs() < 1e-6);
        assert!((usage.iowait_pct - 5.0).abs() < 1e-6);
    }

    #[test]
    fn compute_cpu_delta_handles_zero_and_reset_counters() {
        let same = CpuTimes {
            user: 100,
            idle: 900,
            ..Default::default()
        };
        let (usage, delta) = compute_cpu_delta(&same, &same);
        assert_eq!(delta, 0);
        assert_eq!(usage, CpuUsage::default());

        let smaller = CpuTimes {
            user: 10,
            idle: 20,
            ..Default::default()
        };
        let (usage_reset, delta_reset) = compute_cpu_delta(&same, &smaller);
        assert_eq!(delta_reset, 0);
        assert_eq!(usage_reset, CpuUsage::default());
    }

    #[test]
    fn parse_loadavg_parses_valid_and_rejects_invalid() {
        let (l1, l5, l15, run, total) =
            parse_loadavg("0.42 0.85 1.12 2/512 12345\n").expect("valid loadavg");
        assert!((l1 - 0.42).abs() < 1e-9);
        assert!((l5 - 0.85).abs() < 1e-9);
        assert!((l15 - 1.12).abs() < 1e-9);
        assert_eq!(run, 2);
        assert_eq!(total, 512);

        assert!(parse_loadavg("").is_err());
        assert!(parse_loadavg("0.42 0.85").is_err());
        assert!(parse_loadavg("0.42 0.85 1.12 2-512 12345").is_err());
        assert!(parse_loadavg("NaN 0.85 1.12 2/512 12345").is_err());
    }

    #[test]
    fn parse_uptime_parses_valid_and_rejects_invalid() {
        let secs = parse_uptime("123456.78 987654.32\n").expect("valid uptime");
        assert!((secs - 123_456.78).abs() < 1e-9);

        assert!(parse_uptime("").is_err());
        assert!(parse_uptime("-5.0 10.0").is_err());
        assert!(parse_uptime("inf 10.0").is_err());
        assert!(parse_uptime("abc 10.0").is_err());
    }
}
