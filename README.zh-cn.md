<div align="center">

[English](README.md) | [简体中文](README.zh-cn.md)

</div>

# tg-harvest

Telegram 频道历史消息抽取工具。使用 Rust 编写，通过 [grammers](https://github.com/Lonami/grammers) 库以用户身份登录，从源频道抽取历史消息，按 reaction 数和评论数过滤后转发到目标频道。支持单消息和相册（多图/媒体组）转发。

[![Rust](https://img.shields.io/badge/Rust-1.70+-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)

## 功能

- **历史消息抽取** — 从源频道遍历全部历史消息，按 batch 分批拉取
- **智能过滤** — 支持按 reaction 数量、指定 emoji 或评论数过滤，支持 any/all 组合模式
- **断点续传** — 每个频道独立维护同步游标，崩溃后可从上次位置继续
- **相册转发** — 自动识别并批量转发多图/媒体组消息
- **并行任务** — 支持同时管理多个频道转发任务（最多 3 并发）
- **Flood 保护** — 自动处理 Telegram 限速，支持指数退避重试和 flood_wait 自动等待

## 环境要求

- Rust 1.70+
- Telegram API 凭证（[申请地址](https://my.telegram.org)）

## 快速开始

### 1. 编译

```bash
cargo build --release
```

### 2. 配置 API 凭证

```bash
cp config.toml.example config.toml
```

编辑 `config.toml`:

```toml
api_id = 12345678
api_hash = "your_api_hash_here"

# 全局默认值
forward_delay_ms = 1500
batch_size = 100
```

### 3. 运行

```bash
cargo run
```

首次运行需要输入手机号和验证码完成认证。使用交互式菜单创建频道任务，配置会自动保存到 `channels/` 目录。

## 配置

### 全局配置 (`config.toml`)

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `api_id` | — | Telegram API ID |
| `api_hash` | — | Telegram API Hash |
| `forward_delay_ms` | 1500 | 转发间隔（毫秒） |
| `batch_size` | 100 | 每批拉取消息数 |

### 频道任务配置 (`channels/{source}.toml`)

```toml
source = "channel_username"
target = "target_username"
limit = 0  # 0 = 不限制

[filter]
mode = "any"  # any = 任一条件满足, all = 所有条件都满足

[filter.reactions]
specific = ["👍", "❤"]  # 指定 emoji，为空则统计所有
min_specific_total = 5   # 最低 reaction 总数

[filter.comments]
min = 3  # 最低评论数
```

> [!NOTE]
> 游标从 `last_msg_id + 100` 开始迭代，避免跳过头部消息。即使上一批次部分失败也不会产生遗漏。

## 项目结构

```
tg-harvest/
├── Cargo.toml
├── config.toml              # 全局配置
├── config.toml.example      # 配置模板
├── channels/                # 频道任务配置
│   └── *.toml
├── tg.session              # Telegram 会话文件
├── harvest.db              # SQLite 游标存储
└── src/
    ├── main.rs             # 入口，TUI 主循环，并发调度
    ├── auth.rs             # Telegram 登录（手机号/验证码/2FA）
    ├── config.rs           # 配置解析
    ├── db.rs               # SQLite 游标管理
    ├── harvest.rs          # 消息迭代核心
    ├── filter.rs           # 过滤逻辑
    ├── forward.rs          # 转发逻辑（单消息/相册/重试）
    └── interactive.rs      # 交互式 TUI
```

## 工作流程

1. 运行 `cargo run` 启动交互模式
2. 通过菜单创建频道任务，配置自动保存到 `channels/`
3. 使用已存储的会话连接 Telegram
4. 加载 `harvest.db` 中的游标开始迭代
5. 使用 `iter_messages()` 按 `offset_id` 分批拉取，跳过已处理消息
6. 应用过滤条件（reactions、comments）
7. 将匹配消息转发到目标频道
8. 每批处理后保存游标

## 数据文件

| 文件 | 说明 |
|------|------|
| `config.toml` | 全局配置（api_id, api_hash, 转发延迟等） |
| `channels/*.toml` | 每个频道转发任务的配置 |
| `tg.session` | Telegram 用户会话（sqlite） |
| `harvest.db` | SQLite 数据库，存储同步游标 |

## 关键行为

- **游标偏移** — 从 `last_msg_id + 100` 开始，防止遗漏
- **空批次检测** — 连续 3 次空批次后停止，防止无效翻页
- **转发间隔** — 每次转发间隔 `forward_delay_ms`，防止触发 flood wait
- **Flood 自动恢复** — 解析并自动等待 `FLOOD_WAIT_{N}`
- **权限检查** — 目标频道缺少管理员权限时任务终止
- **相册原子性** — 相册作为整体处理，要么全部转发，要么全部跳过
