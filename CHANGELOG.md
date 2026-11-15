# Changelog

## [0.1.1] - 2026-11-15

### Fixed
- Run status message expiration on every event loop iteration and clamp poll timeout to remaining TTL.
- Sanitize control characters and ANSI/OSC escape sequences from `/proc` process strings and `/etc/passwd` usernames.
- Guard `send_signal` against non-positive PIDs and verify target `comm` before delivering signals from the confirmation modal.
- Harden `install.sh` ancestor-directory writability checks and fail closed when no SHA-256 tool is available.
- Thread `clock_ticks_per_sec` from `SystemSnapshot` into `format_cpu_time`, make `ui::render` read-only, remove dead `AppAction::RefreshNow` and unused collector fields/methods, and eliminate per-frame `Vec` allocations in `aggregate_net_rates`.

## [0.1.0] - 2026-11-15

### Added
- Linux `/proc` collectors for CPU (aggregate & per-core), memory/swap, load average/uptime, network RX/TX rates, and process metrics with delta CPU% and UID resolution.
- Interactive `ratatui` + `crossterm` TUI with header meters/sparklines, sortable & filterable process table, tree view with collapse/expand, signal confirmation modal, and help overlay.
- CLI flags for refresh delay, initial sort column, tree mode, user filter, text filter, and `--no-color`.
- Release build profile (`opt-level = 3`, thin LTO, single codegen unit, symbol stripping) and GitHub Actions CI & release workflows.
