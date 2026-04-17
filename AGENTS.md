# AGENTS.md

## Project Overview
用 Rust 编写的 Telegram 频道历史消息抽取工具。通过 grammers 库以用户账号身份登录，从配置的源频道抽取历史消息，按 reaction 数和评论数过滤后转发到目标频道。支持单个消息和相册（多图/媒体组）转发。每个频道独立维护同步游标，支持断点续传。

## Tech Stack
Rust, Cargo, grammers, grammers-session (sqlite-storage), tokio, rusqlite (bundled), serde, toml, chrono, anyhow, tracing, tracing-subscriber, dialoguer, rand

## Project Structure
- `src/main.rs` — 入口，交互式 TUI 主循环，并发任务调度（semaphore 控制最大 3 并发）
- `src/auth.rs` — Telegram 登录（手机号验证码 / 2FA），Session 持久化到 `tg.session`
- `src/config.rs` — 配置解析：全局配置 (`config.toml`) 和频道任务配置 (`channels/*.toml`)
- `src/db.rs` — SQLite 游标管理 (`harvest.db`)，表 `channel_cursors` 存储每个 source→target 的同步进度
- `src/harvest.rs` — 消息迭代核心：从源频道按 offset_id 分批拉取，支持相册（grouped_id）去重，跳过已转发消息，按游标断点续传
- `src/filter.rs` — 过滤逻辑：`ReactionFilter`（指定 emoji 或总量）、`CommentFilter`（最低评论数），支持 any/all 组合模式
- `src/forward.rs` — 转发逻辑：单消息转发和相册转发（多消息批量 forward_messages），指数退避重试 + flood_wait 自动等待
- `src/interactive.rs` — 交互式 TUI：列出频道/群组、选择源/目标、配置过滤条件、增删改任务配置

## Data Files
- `config.toml` — 全局配置（api_id, api_hash, forward_delay_ms, batch_size）
- `channels/*.toml` — 每个频道转发任务的配置（source, target, limit, filter）
- `harvest.db` — SQLite 数据库，游标记录
- `tg.session` — Telegram 会话文件

## Config Format (channels/{source}.toml)
```toml
source = "channel_username"
target = "target_username"
limit = 0  # 0 = 不限制

[filter]
mode = "any"  # any | all

[filter.reactions]
specific = ["👍", "❤️"]  # 指定 emoji，为空则统计所有
min_specific_total = 5

[filter.comments]
min = 3
```

## Build Commands
cargo build, cargo run, cargo test

## Key Behaviors
- 游标从 `last_msg_id + 100` 开始迭代，避免跳过头部消息
- 空批次连续 3 次则停止（防止无效翻页）
- 一次迭代最多拉取 `latest_msg_id + 100`，防止无限向前
- 转发间隔 `forward_delay_ms`（默认 1500ms），防止触发 flood wait
- flood wait 自动解析并等待，不中断任务
- 目标频道缺少管理员权限时任务终止

## Language
中文回复。

## Working Style
保持回复简洁。

## Python Skill
如果项目有 python skill，使用 .venv 目录下的 python。