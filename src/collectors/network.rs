use super::{CollectorError, NetInterfaceStats};

/// Raw cumulative counters for a single network interface from `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRaw {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
}

/// Parses `/proc/net/dev` content into a list of `NetRaw` entries.
pub fn parse_net_dev(content: &str) -> Result<Vec<NetRaw>, CollectorError> {
    let mut lines = content.lines().filter(|l| !l.trim().is_empty());

    // Skip the two /proc/net/dev header lines.
    if lines.next().is_none() || lines.next().is_none() {
        return Err(CollectorError::Parse(
            "missing header lines in /proc/net/dev".into(),
        ));
    }

    let mut interfaces = Vec::new();
    for line in lines {
        let Some((iface, rest)) = line.split_once(':') else {
            return Err(CollectorError::Parse(format!(
                "missing ':' separator in /proc/net/dev line: '{line}'"
            )));
        };

        let name = iface.trim();
        if name.is_empty() {
            return Err(CollectorError::Parse(
                "empty interface name in /proc/net/dev".into(),
            ));
        }

        let fields: Vec<u64> = rest
            .split_whitespace()
            .take(10)
            .map(|tok| {
                tok.parse::<u64>().map_err(|e| {
                    CollectorError::Parse(format!(
                        "invalid counter '{tok}' for interface '{name}': {e}"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if fields.len() < 10 {
            return Err(CollectorError::Parse(format!(
                "expected at least 10 counter fields for interface '{name}', found {}",
                fields.len()
            )));
        }

        interfaces.push(NetRaw {
            name: name.to_string(),
            rx_bytes: fields[0],
            rx_packets: fields[1],
            tx_bytes: fields[8],
            tx_packets: fields[9],
        });
    }

    Ok(interfaces)
}

/// Computes per-interface byte/sec rates between `prev` and `curr` over `elapsed_secs`.
///
/// Uses `saturating_sub` to safely handle counter resets/wraps, and returns `0.0` rates
/// for newly seen interfaces or when `elapsed_secs` is zero, negative, near-zero, or non-finite.
pub fn compute_net_rates(
    prev: &[NetRaw],
    curr: &[NetRaw],
    elapsed_secs: f64,
) -> Vec<NetInterfaceStats> {
    let valid_elapsed = elapsed_secs.is_finite() && elapsed_secs > 1e-6;

    curr.iter()
        .map(|c| {
            let (rx_bytes_per_sec, tx_bytes_per_sec) = if valid_elapsed {
                if let Some(p) = prev.iter().find(|p| p.name == c.name) {
                    let rx_delta = c.rx_bytes.saturating_sub(p.rx_bytes);
                    let tx_delta = c.tx_bytes.saturating_sub(p.tx_bytes);
                    (
                        rx_delta as f64 / elapsed_secs,
                        tx_delta as f64 / elapsed_secs,
                    )
                } else {
                    (0.0, 0.0)
                }
            } else {
                (0.0, 0.0)
            };

            NetInterfaceStats {
                name: c.name.clone(),
                rx_bytes: c.rx_bytes,
                tx_bytes: c.tx_bytes,
                rx_packets: c.rx_packets,
                tx_packets: c.tx_packets,
                rx_bytes_per_sec,
                tx_bytes_per_sec,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1234567   12345    0    0    0     0          0         0  1234567   12345    0    0    0     0       0          0
  eth0: 987654321  654321   1    2    0     0          0        10 111222333  123456    0    0    0     0       0          0
";

    #[test]
    fn parse_net_dev_parses_interfaces() {
        let parsed = parse_net_dev(SAMPLE_NET_DEV).expect("valid /proc/net/dev");
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].name, "lo");
        assert_eq!(parsed[0].rx_bytes, 1_234_567);
        assert_eq!(parsed[0].rx_packets, 12_345);
        assert_eq!(parsed[0].tx_bytes, 1_234_567);
        assert_eq!(parsed[0].tx_packets, 12_345);

        assert_eq!(parsed[1].name, "eth0");
        assert_eq!(parsed[1].rx_bytes, 987_654_321);
        assert_eq!(parsed[1].rx_packets, 654_321);
        assert_eq!(parsed[1].tx_bytes, 111_222_333);
        assert_eq!(parsed[1].tx_packets, 123_456);
    }

    #[test]
    fn parse_net_dev_rejects_empty_or_malformed() {
        assert!(parse_net_dev("").is_err());
        assert!(parse_net_dev("Inter-| Receive\n").is_err());
        assert!(parse_net_dev("h1\nh2\neth0 no_colon 1 2 3\n").is_err());
        assert!(parse_net_dev("h1\nh2\neth0: 1 2 3 4\n").is_err());
        assert!(parse_net_dev("h1\nh2\n: 1 2 3 4 5 6 7 8 9 10\n").is_err());
    }

    #[test]
    fn compute_net_rates_calculates_rates_and_handles_first_sample_and_wrap() {
        let prev = vec![
            NetRaw {
                name: "eth0".into(),
                rx_bytes: 1000,
                rx_packets: 10,
                tx_bytes: 500,
                tx_packets: 5,
            },
            NetRaw {
                name: "wlan0".into(),
                rx_bytes: 5000,
                rx_packets: 50,
                tx_bytes: 2000,
                tx_packets: 20,
            },
        ];
        let curr = vec![
            NetRaw {
                name: "eth0".into(),
                rx_bytes: 3000, // +2000 over 2s = 1000 B/s
                rx_packets: 30,
                tx_bytes: 1500, // +1000 over 2s = 500 B/s
                tx_packets: 15,
            },
            NetRaw {
                name: "wlan0".into(),
                rx_bytes: 100, // counter wrapped/reset (< 5000) -> saturating_sub = 0
                rx_packets: 1,
                tx_bytes: 3000, // +1000 over 2s = 500 B/s
                tx_packets: 30,
            },
            NetRaw {
                name: "docker0".into(), // newly appeared -> 0.0 rates
                rx_bytes: 4096,
                rx_packets: 40,
                tx_bytes: 1024,
                tx_packets: 10,
            },
        ];

        // First sample (empty prev)
        let first = compute_net_rates(&[], &curr, 2.0);
        assert_eq!(first.len(), 3);
        assert_eq!(first[0].rx_bytes_per_sec, 0.0);
        assert_eq!(first[0].tx_bytes_per_sec, 0.0);

        // Normal delta + wrap + new interface
        let rates = compute_net_rates(&prev, &curr, 2.0);
        assert_eq!(rates.len(), 3);
        assert!((rates[0].rx_bytes_per_sec - 1000.0).abs() < 1e-6);
        assert!((rates[0].tx_bytes_per_sec - 500.0).abs() < 1e-6);
        assert_eq!(rates[1].rx_bytes_per_sec, 0.0);
        assert!((rates[1].tx_bytes_per_sec - 500.0).abs() < 1e-6);
        assert_eq!(rates[2].rx_bytes_per_sec, 0.0);
        assert_eq!(rates[2].tx_bytes_per_sec, 0.0);

        // Zero / near-zero elapsed time
        let zero_elapsed = compute_net_rates(&prev, &curr, 0.0);
        assert_eq!(zero_elapsed[0].rx_bytes_per_sec, 0.0);
        assert_eq!(zero_elapsed[0].tx_bytes_per_sec, 0.0);

        let tiny_elapsed = compute_net_rates(&prev, &curr, 1e-9);
        assert_eq!(tiny_elapsed[0].rx_bytes_per_sec, 0.0);
    }
}
