<div align="center">

[English](README.md) | [简体中文](README.zh-cn.md)

</div>

# tg-harvest

Telegram channel history message harvesting tool. Written in Rust, uses the [grammers](https://github.com/Lonami/grammers) library to log in as a user account, extract historical messages from source channels, filter by reaction count and comment count, and forward to target channels. Supports single messages and albums (multi-image/media groups).

[![Rust](https://img.shields.io/badge/Rust-1.70+-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)

## Features

- **History Harvesting** — Iterate through all historical messages from source channels in batches
- **Smart Filtering** — Filter by reaction count, specific emoji, or comment count, with any/all combination modes
- **Resume from Breakpoint** — Each channel maintains an independent sync cursor, resumable after crashes
- **Album Forwarding** — Auto-detect and batch forward multi-image/media group messages
- **Parallel Tasks** — Manage multiple channel forwarding tasks simultaneously (up to 3 concurrent)
- **Flood Protection** — Auto-handle Telegram rate limits with exponential backoff retry and flood_wait auto-wait

## Requirements

- Rust 1.70+
- Telegram API credentials ([Get them here](https://my.telegram.org))

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Configure API Credentials

```bash
cp config.toml.example config.toml
```

Edit `config.toml`:

```toml
api_id = 12345678
api_hash = "your_api_hash_here"

# Global defaults
forward_delay_ms = 1500
batch_size = 100
```

### 3. Run

```bash
cargo run
```

On first run, enter your phone number and verification code to authenticate. Use the interactive menu to create channel tasks — configs are auto-generated to `channels/`.

## Configuration

### Global Config (`config.toml`)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `api_id` | — | Telegram API ID |
| `api_hash` | — | Telegram API Hash |
| `forward_delay_ms` | 1500 | Delay between forwards (ms) |
| `batch_size` | 100 | Messages per batch |

### Channel Task Config (`channels/{source}.toml`)

```toml
source = "channel_username"
target = "target_username"
limit = 0  # 0 = unlimited

[filter]
mode = "any"  # any = any condition passes, all = all conditions must pass

[filter.reactions]
specific = ["👍", "❤️"]  # specific emoji to count (empty = count all)
min_specific_total = 5   # minimum total reactions

[filter.comments]
min = 3  # minimum comment count
```

> [!NOTE]
> Cursor starts at `last_msg_id + 100` to avoid skipping head messages. No gaps even if previous batch partially failed.

## Project Structure

```
tg-harvest/
├── Cargo.toml
├── config.toml              # Global config
├── config.toml.example      # Config template
├── channels/                # Channel task configs
│   └── *.toml
├── tg.session              # Telegram session file
├── harvest.db              # SQLite cursor storage
└── src/
    ├── main.rs             # Entry, TUI main loop, concurrency
    ├── auth.rs             # Telegram login (phone/code/2FA)
    ├── config.rs           # Config parsing
    ├── db.rs               # SQLite cursor management
    ├── harvest.rs          # Core message iteration
    ├── filter.rs           # Filter logic
    ├── forward.rs          # Forward logic (single/album/retry)
    └── interactive.rs      # Interactive TUI
```

## How It Works

1. Run `cargo run` to start interactive mode
2. Create channel tasks via menu — configs auto-generated to `channels/`
3. Connect to Telegram using stored session
4. Load cursor from `harvest.db` and start iteration
5. Use `iter_messages()` to fetch by `offset_id` batches, skip already processed
6. Apply filter conditions (reactions, comments)
7. Forward matching messages to target channel
8. Save cursor after each batch

## Data Files

| File | Purpose |
|------|---------|
| `config.toml` | Global config (api_id, api_hash, delay, etc.) |
| `channels/*.toml` | Per-task channel forwarding configs |
| `tg.session` | Telegram user session (sqlite) |
| `harvest.db` | SQLite database for sync cursors |

## Key Behaviors

- **Cursor offset** — Starts at `last_msg_id + 100` to prevent gaps
- **Empty batch detection** — Stops after 3 consecutive empty batches
- **Forward delay** — `forward_delay_ms` between forwards to prevent flood wait
- **Flood auto-recovery** — Parses and auto-waits `FLOOD_WAIT_{N}`
- **Permission check** — Task aborts if target lacks admin rights
- **Album atomicity** — Albums processed as a unit — all or nothing
