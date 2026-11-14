# Changelog

## [0.1.0] - 2026-11-15

### Added
- Linux `/proc` collectors for CPU (aggregate & per-core), memory/swap, load average/uptime, network RX/TX rates, and process metrics with delta CPU% and UID resolution.
- Interactive `ratatui` + `crossterm` TUI with header meters/sparklines, sortable & filterable process table, tree view with collapse/expand, signal confirmation modal, and help overlay.
- CLI flags for refresh delay, initial sort column, tree mode, user filter, text filter, and `--no-color`.
- Release build profile (`opt-level = 3`, thin LTO, single codegen unit, symbol stripping) and GitHub Actions CI & release workflows.
