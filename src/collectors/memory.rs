use super::{CollectorError, MemStats};

/// Parses `/proc/meminfo` content into `MemStats` using htop/procps accounting.
///
/// Formulas:
/// - `cached = Cached + SReclaimable - Shmem` (saturating)
/// - `used = MemTotal - (MemFree + Cached + SReclaimable + Buffers)` with saturating
///   fallback to `MemTotal - MemFree`
/// - `swap_used = SwapTotal - SwapFree - SwapCached` (saturating)
pub fn parse_meminfo(content: &str) -> Result<MemStats, CollectorError> {
    let mut total_kb: Option<u64> = None;
    let mut free_kb = 0u64;
    let mut buffers_kb = 0u64;
    let mut cached_raw = 0u64;
    let mut sreclaimable_kb = 0u64;
    let mut shared_kb = 0u64;
    let mut swap_total_kb = 0u64;
    let mut swap_cached_kb = 0u64;
    let mut swap_free_kb = 0u64;

    for line in content.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(val_str) = rest.split_whitespace().next() else {
            continue;
        };
        let val = val_str.parse::<u64>().map_err(|e| {
            CollectorError::Parse(format!("invalid meminfo value for '{key}': {e}"))
        })?;

        match key {
            "MemTotal" => total_kb = Some(val),
            "MemFree" => free_kb = val,
            "Buffers" => buffers_kb = val,
            "Cached" => cached_raw = val,
            "SReclaimable" => sreclaimable_kb = val,
            "Shmem" => shared_kb = val,
            "SwapTotal" => swap_total_kb = val,
            "SwapCached" => swap_cached_kb = val,
            "SwapFree" => swap_free_kb = val,
            _ => {}
        }
    }

    let total_kb = total_kb
        .ok_or_else(|| CollectorError::Parse("missing 'MemTotal' in /proc/meminfo".into()))?;

    let cached_kb = cached_raw
        .saturating_add(sreclaimable_kb)
        .saturating_sub(shared_kb);

    let used_diff = free_kb
        .saturating_add(cached_raw)
        .saturating_add(sreclaimable_kb)
        .saturating_add(buffers_kb);
    let used_kb = if total_kb >= used_diff {
        total_kb - used_diff
    } else {
        total_kb.saturating_sub(free_kb)
    };

    let swap_used_kb = swap_total_kb
        .saturating_sub(swap_free_kb)
        .saturating_sub(swap_cached_kb);

    Ok(MemStats {
        total_kb,
        used_kb,
        free_kb,
        buffers_kb,
        cached_kb,
        swap_total_kb,
        swap_used_kb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MEMINFO: &str = "\
MemTotal:       16384000 kB
MemFree:         2048000 kB
MemAvailable:    8192000 kB
Buffers:          512000 kB
Cached:          5000000 kB
SwapCached:       100000 kB
Active:          7000000 kB
Inactive:        4000000 kB
SwapTotal:       4096000 kB
SwapFree:        3000000 kB
Shmem:            400000 kB
SReclaimable:     600000 kB
SUnreclaim:       200000 kB
";

    #[test]
    fn parse_meminfo_computes_htop_fields() {
        let stats = parse_meminfo(SAMPLE_MEMINFO).expect("valid /proc/meminfo");
        assert_eq!(stats.total_kb, 16_384_000);
        assert_eq!(stats.free_kb, 2_048_000);
        assert_eq!(stats.buffers_kb, 512_000);
        // cached = Cached(5_000_000) + SReclaimable(600_000) - Shmem(400_000) = 5_200_000
        assert_eq!(stats.cached_kb, 5_200_000);
        // used_diff = 2_048_000 + 5_000_000 + 600_000 + 512_000 = 8_160_000
        // used = 16_384_000 - 8_160_000 = 8_224_000
        assert_eq!(stats.used_kb, 8_224_000);
        assert_eq!(stats.swap_total_kb, 4_096_000);
        // swap_used = 4_096_000 - 3_000_000 - 100_000 = 996_000
        assert_eq!(stats.swap_used_kb, 996_000);
    }

    #[test]
    fn parse_meminfo_fallback_when_used_diff_exceeds_total_or_available_missing() {
        let meminfo = "\
MemTotal:       1000 kB
MemFree:         400 kB
Buffers:         300 kB
Cached:          500 kB
Shmem:           600 kB
SwapTotal:       500 kB
SwapFree:        400 kB
SwapCached:      200 kB
";
        let stats = parse_meminfo(meminfo).expect("valid fallback /proc/meminfo");
        // used_diff = 400 + 500 + 0 + 300 = 1200 > 1000 -> fallback total - free = 600
        assert_eq!(stats.used_kb, 600);
        // cached = (500 + 0).saturating_sub(600) = 0
        assert_eq!(stats.cached_kb, 0);
        // swap_used = 500 - 400 - 200 saturating = 0
        assert_eq!(stats.swap_used_kb, 0);
    }

    #[test]
    fn parse_meminfo_rejects_empty_or_missing_total_or_bad_number() {
        assert!(parse_meminfo("").is_err());
        assert!(parse_meminfo("MemFree: 1000 kB\n").is_err());
        assert!(parse_meminfo("MemTotal: not_a_num kB\n").is_err());
    }
}
