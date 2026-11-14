# rstop

Fast, interactive Linux terminal system monitor written in Rust, inspired by `htop` and `btop`.

## Features

- **System & CPU meters**: Total and per-core CPU usage, CPU history sparkline, load average, uptime, memory/swap usage, and network RX/TX rates from `/proc`.
- **Process table**: Sortable by CPU%, MEM%, PID, user, time, or command; live text filtering and user filtering; flat and tree (`t`) views with collapse/expand.
- **Process signals**: Send `SIGTERM` / `SIGKILL` (and other signals) via an interactive confirmation modal (`F9` / `x` / `K`).
- **Configurable CLI**: Custom refresh interval, initial sort column, tree mode, user/text filters, and `--no-color` support.

## Requirements

- Linux (reads `/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/uptime`, `/proc/net/dev`, and `/proc/[pid]/*`)
- Rust toolchain (edition 2024)

## Build & Run

```bash
cargo build --release
./target/release/rstop
```

## Usage

```text
Usage: rstop [OPTIONS]

Options:
  -d, --delay <MS>      Refresh interval in milliseconds (100..=60000) [default: 1000]
  -s, --sort <COLUMN>   Initial sort column [default: cpu] [possible values: cpu, mem, pid, user, time, command]
  -t, --tree            Start in process tree view
  -u, --user <USER>     Filter processes by username
  -f, --filter <QUERY>  Initial process name/cmdline filter
      --no-color        Disable color styling
  -h, --help            Print help
  -V, --version         Print version
```

### Keybindings

- `q` / `F10` / `Ctrl+C`: Quit
- `Up`/`Down` or `k`/`j`, `PgUp`/`PgDn`, `Home`/`End` or `g`/`G`: Navigate process list
- `s` / `<` / `>`: Cycle sort column; `r` / `I`: Reverse sort order
- `t` / `F5`: Toggle process tree view (`Enter`/`Space` to collapse/expand)
- `/` / `F3`: Filter processes
- `F9` / `x` / `K`: Open signal modal (`SIGTERM`, `SIGKILL`, etc.)
- `?` / `F1`: Toggle help overlay
