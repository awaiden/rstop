use clap::{Parser, ValueEnum};

/// Process table sort column.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Cpu,
    Mem,
    Pid,
    User,
    Time,
    Command,
}

/// Command-line arguments for `rstop`.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(name = "rstop", version, about = "Interactive Linux system monitor")]
pub struct CliArgs {
    /// Refresh interval in milliseconds (100..=60000).
    #[arg(
        short = 'd',
        long,
        default_value_t = 1000,
        value_name = "MS",
        value_parser = clap::value_parser!(u64).range(100..=60_000)
    )]
    pub delay: u64,

    /// Initial sort column.
    #[arg(short = 's', long, value_enum, default_value_t = SortColumn::Cpu, value_name = "COLUMN")]
    pub sort: SortColumn,

    /// Start in process tree view.
    #[arg(short = 't', long)]
    pub tree: bool,

    /// Filter processes by username.
    #[arg(short = 'u', long, value_name = "USER")]
    pub user: Option<String>,

    /// Initial process name/cmdline filter.
    #[arg(short = 'f', long, value_name = "QUERY")]
    pub filter: Option<String>,

    /// Disable color styling.
    #[arg(long)]
    pub no_color: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_applied() {
        let args = CliArgs::try_parse_from(["rstop"]).expect("default args should parse");
        assert_eq!(args.delay, 1000);
        assert_eq!(args.sort, SortColumn::Cpu);
        assert!(!args.tree);
        assert_eq!(args.user, None);
        assert_eq!(args.filter, None);
        assert!(!args.no_color);
    }

    #[test]
    fn short_flags_parse() {
        let args = CliArgs::try_parse_from([
            "rstop", "-d", "250", "-s", "mem", "-t", "-u", "alice", "-f", "sshd",
        ])
        .expect("short flags should parse");
        assert_eq!(args.delay, 250);
        assert_eq!(args.sort, SortColumn::Mem);
        assert!(args.tree);
        assert_eq!(args.user.as_deref(), Some("alice"));
        assert_eq!(args.filter.as_deref(), Some("sshd"));
        assert!(!args.no_color);
    }

    #[test]
    fn long_flags_parse() {
        let args = CliArgs::try_parse_from([
            "rstop",
            "--delay",
            "60000",
            "--sort",
            "command",
            "--tree",
            "--user",
            "root",
            "--filter",
            "systemd",
            "--no-color",
        ])
        .expect("long flags should parse");
        assert_eq!(args.delay, 60_000);
        assert_eq!(args.sort, SortColumn::Command);
        assert!(args.tree);
        assert_eq!(args.user.as_deref(), Some("root"));
        assert_eq!(args.filter.as_deref(), Some("systemd"));
        assert!(args.no_color);
    }

    #[test]
    fn delay_bounds_are_enforced() {
        assert!(CliArgs::try_parse_from(["rstop", "-d", "100"]).is_ok());
        assert!(CliArgs::try_parse_from(["rstop", "-d", "60000"]).is_ok());
        assert!(CliArgs::try_parse_from(["rstop", "-d", "99"]).is_err());
        assert!(CliArgs::try_parse_from(["rstop", "-d", "60001"]).is_err());
        assert!(CliArgs::try_parse_from(["rstop", "-d", "0"]).is_err());
        assert!(CliArgs::try_parse_from(["rstop", "-d", "abc"]).is_err());
    }

    #[test]
    fn sort_column_variants_parse() {
        let cases = [
            ("cpu", SortColumn::Cpu),
            ("mem", SortColumn::Mem),
            ("pid", SortColumn::Pid),
            ("user", SortColumn::User),
            ("time", SortColumn::Time),
            ("command", SortColumn::Command),
        ];

        for (raw, expected) in cases {
            let args = CliArgs::try_parse_from(["rstop", "--sort", raw])
                .unwrap_or_else(|e| panic!("failed to parse sort={raw}: {e}"));
            assert_eq!(args.sort, expected);
        }

        assert!(CliArgs::try_parse_from(["rstop", "--sort", "invalid"]).is_err());
        assert_eq!(SortColumn::default(), SortColumn::Cpu);
    }
}
