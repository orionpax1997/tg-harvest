mod config;
mod db;
mod auth;
mod harvest;
mod filter;
mod forward;
mod interactive;

use config::GlobalConfig;
use interactive::{
    list_channels, select_from_list, ask_filter_settings, create_channel_config,
    update_channel_config, delete_channel_config, list_existing_configs, get_channel_display_name,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const MAX_CONCURRENT_TASKS: usize = 3;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    tracing::info!("tg-harvest starting");

    let global_config = GlobalConfig::load_default().context("Failed to load global config")?;
    tracing::info!(
        "Config: delay={}ms, batch={}",
        global_config.forward_delay_ms,
        global_config.batch_size
    );

    let session_path = PathBuf::from("tg.session");
    let client = if session_path.exists() {
        auth::create_client(
            global_config.api_id,
            &session_path,
        )
        .await
        .context("Failed to create client from existing session")?
    } else {
        println!("No session file found. Starting authentication...");
        auth::login_and_create_client(
            global_config.api_id,
            &global_config.api_hash,
            &session_path,
        )
        .await
        .context("Failed to authenticate. Please ensure your API credentials are correct.")?
    };

    tracing::info!("Telegram connected");

    let db_path = PathBuf::from("harvest.db");
    let db_conn = Arc::new(Mutex::new(db::init_db(&db_path).context("Failed to initialize database")?));
    tracing::info!("DB ready");

    loop {
        let existing_configs = list_existing_configs()?;
        if existing_configs.is_empty() {
            println!("\n=== 设置频道转发任务 ===");
            println!("正在加载你的频道和群组列表...");
            
            let all_channels = list_channels(&client).await?;
            if all_channels.is_empty() {
                println!("未找到任何频道或群组。");
                return Ok(());
            }

            println!("共找到 {} 个频道/群组", all_channels.len());

            let source_idx = select_from_list(
                &all_channels,
                "选择源频道（从中抽取消息）"
            )?;
            let source = &all_channels[source_idx];
            
            let target_idx = select_from_list(
                &all_channels,
                "选择目标频道（转发消息到此处）"
            )?;
            let target = &all_channels[target_idx];

            let source_name = source.username.clone().unwrap_or_else(|| source.id.to_string());
            let target_name = target.username.clone().unwrap_or_else(|| target.id.to_string());

            let filter = ask_filter_settings()?;

            println!("\n确认转发配置:");
            println!("  源: {} ({})", source.name, source_name);
            println!("  目标: {} ({})", target.name, target_name);
            println!("  过滤: 模式={}, 最低reactions={}, 最低评论={}", filter.mode, filter.min_reactions, filter.min_comments);

            let confirm = dialoguer::Confirm::new()
                .with_prompt("确认创建此转发任务?")
                .interact()?;

            if confirm {
                create_channel_config(&source_name, &target_name, &filter)?;
                println!("配置已创建！");
                continue;
            } else {
                println!("已取消。");
                continue;
            }
        }

        println!("\n=== 转发任务管理 ===");
        let mut task_items = Vec::new();
        for cfg in existing_configs.iter() {
            let limit_str = if cfg.limit == 0 { "全部" } else { &cfg.limit.to_string() };
            let filter_desc = if let Some(ref r) = cfg.filter.reactions {
                format!("reactions>={}", r.min_specific_total)
            } else if let Some(ref c) = cfg.filter.comments {
                format!("comments>={}", c.min)
            } else {
                "无过滤".to_string()
            };
            let source_name = get_channel_display_name(&client, &cfg.source).await;
            let target_name = get_channel_display_name(&client, &cfg.target).await;
            let item = format!("{} -> {} (限制: {}, 过滤: {})", source_name, target_name, limit_str, filter_desc);
            task_items.push(item.clone());
        }
        task_items.push("并行执行所有任务".to_string());
        task_items.push("添加新任务".to_string());
        task_items.push("退出".to_string());

        let selection = dialoguer::Select::new()
            .with_prompt("选择任务")
            .items(&task_items)
            .default(0)
            .interact()?;

        if selection == existing_configs.len() {
            // 并行执行所有任务
            println!("\n=== 并行执行所有 {} 个任务 ===", existing_configs.len());
            println!("最大并发数: {}", MAX_CONCURRENT_TASKS);
            
            let confirm = dialoguer::Confirm::new()
                .with_prompt("确认开始并行执行?")
                .interact()?;
            
            if !confirm {
                println!("已取消。");
                continue;
            }

            let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_TASKS));
            let mut join_set = tokio::task::JoinSet::new();
            let mut total_scanned = 0i64;
            let mut total_forwarded = 0i64;
            let mut total_skipped = 0i64;

            for cfg in existing_configs.iter() {
                let client = client.clone();
                let db_conn = Arc::clone(&db_conn);
                let cfg = cfg.clone();
                let global_config = global_config.clone();
                let permit = Arc::clone(&semaphore);
                
                join_set.spawn(async move {
                    let _permit = permit.acquire().await.unwrap();
                    tracing::info!("Task: {} -> {}", cfg.source, cfg.target);
                    
                    let result = harvest::harvest_channel(&client, &db_conn, &cfg, &global_config).await;
                    (cfg.source.clone(), result)
                });
            }

            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok((source, Ok(stats))) => {
                        total_scanned += stats.total_scanned;
                        total_forwarded += stats.total_forwarded;
                        total_skipped += stats.total_skipped;
                        tracing::info!(
                            "Done {}: +{}/{} forwarded, {} skipped",
                            source, stats.total_forwarded, stats.total_scanned, stats.total_skipped
                        );
                    }
                    Ok((source, Err(e))) => {
                        tracing::error!("Failed '{}': {}", source, e);
                    }
                    Err(e) => {
                        tracing::error!("Task panicked: {}", e);
                    }
                }
            }

            println!("\n========== SUMMARY ==========");
            println!("Total scanned:   {}", total_scanned);
            println!("Total forwarded: {}", total_forwarded);
            println!("Total skipped:   {}", total_skipped);
            println!("============================");
            continue;
        } else if selection == existing_configs.len() + 1 {
            // 添加新任务
            println!("\n=== 添加新转发任务 ===");
            println!("正在加载你的频道和群组列表...");
            
            let all_channels = list_channels(&client).await?;
            if all_channels.is_empty() {
                println!("未找到任何频道或群组。");
                return Ok(());
            }
            
            println!("共找到 {} 个频道/群组", all_channels.len());

            let source_idx = select_from_list(
                &all_channels,
                "选择源频道（从中抽取消息）"
            )?;
            let source = &all_channels[source_idx];
            
            let target_idx = select_from_list(
                &all_channels,
                "选择目标频道（转发消息到此处）"
            )?;
            let target = &all_channels[target_idx];

            let source_name = source.username.clone().unwrap_or_else(|| source.id.to_string());
            let target_name = target.username.clone().unwrap_or_else(|| target.id.to_string());

            let filter = ask_filter_settings()?;

            println!("\n确认转发配置:");
            println!("  源: {} ({})", source.name, source_name);
            println!("  目标: {} ({})", target.name, target_name);
            println!("  过滤: 模式={}, 最低reactions={}, 最低评论={}", filter.mode, filter.min_reactions, filter.min_comments);

            let confirm = dialoguer::Confirm::new()
                .with_prompt("确认创建此转发任务?")
                .interact()?;

            if confirm {
                create_channel_config(&source_name, &target_name, &filter)?;
                println!("配置已创建！");
                continue;
            } else {
                println!("已取消。");
                continue;
            }
        } else if selection == existing_configs.len() + 2 {
            println!("已退出。");
            return Ok(());
        }

        // 选中了已有任务，显示子菜单
        let channel_config = &existing_configs[selection];
        let task_actions = vec![
            "执行任务（开始转发）",
            "编辑过滤条件",
            "删除任务",
            "返回上级菜单",
        ];

        let action = dialoguer::Select::new()
            .with_prompt(format!("任务: {} -> {}", channel_config.source, channel_config.target))
            .items(&task_actions)
            .default(0)
            .interact()?;

        match action {
            0 => {
                // 执行任务
                tracing::info!(
                    "Processing {} -> {} (limit: {})",
                    channel_config.source,
                    channel_config.target,
                    if channel_config.limit == 0 {
                        "all".to_string()
                    } else {
                        channel_config.limit.to_string()
                    }
                );

                let mut total_scanned = 0i64;
                let mut total_forwarded = 0i64;
                let mut total_skipped = 0i64;

                match harvest::harvest_channel(&client, &db_conn, channel_config, &global_config).await {
                    Ok(stats) => {
                        total_scanned += stats.total_scanned;
                        total_forwarded += stats.total_forwarded;
                        total_skipped += stats.total_skipped;
                        tracing::info!(
                            "Done {}: +{}/{} forwarded, {} skipped",
                            channel_config.source,
                            stats.total_forwarded,
                            stats.total_scanned,
                            stats.total_skipped
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to harvest channel '{}': {}",
                            channel_config.source,
                            e
                        );
                    }
                }

                println!("\n========== SUMMARY ==========");
                println!("Total scanned:   {}", total_scanned);
                println!("Total forwarded: {}", total_forwarded);
                println!("Total skipped:   {}", total_skipped);
                println!("============================");

                tracing::info!(
                    "Total: +{}/{} forwarded, {} skipped",
                    total_forwarded,
                    total_scanned,
                    total_skipped
                );
            }
            1 => {
                // 编辑过滤条件
                println!("\n=== 编辑过滤条件 ===");
                println!("当前配置:");
                println!("  模式: {}", channel_config.filter.mode);
                if let Some(ref r) = channel_config.filter.reactions {
                    println!("  最低 reactions: {}", r.min_specific_total);
                }
                if let Some(ref c) = channel_config.filter.comments {
                    println!("  最低评论: {}", c.min);
                }

                let filter = ask_filter_settings()?;

                let confirm = dialoguer::Confirm::new()
                    .with_prompt("确认更新过滤条件?")
                    .interact()?;

                if confirm {
                    update_channel_config(
                        &channel_config.source,
                        &channel_config.target,
                        &filter,
                    )?;
                } else {
                    println!("已取消。");
                }
            }
            2 => {
                // 删除任务
                let confirm = dialoguer::Confirm::new()
                    .with_prompt(format!("确认删除任务 {} -> {}?", channel_config.source, channel_config.target))
                    .interact()?;

                if confirm {
                    delete_channel_config(&channel_config.source)?;
                    {
                        let db = db_conn.lock().await;
                        let _ = db::delete_cursor(&db, &channel_config.source, &channel_config.target);
                    }
                    println!("已删除数据库游标记录");
                } else {
                    println!("已取消。");
                }
            }
            3 => {
                // 返回
                continue;
            }
            _ => {}
        }
    }
}

fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tg_harvest=debug"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .init();
}
