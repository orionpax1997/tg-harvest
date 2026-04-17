use grammers_client::{Client, SignInError};
use grammers_session::storages::SqliteSession;
use std::sync::Arc;
use anyhow::Context;

pub async fn create_client(
    api_id: i32,
    session_path: &std::path::Path,
) -> anyhow::Result<Client> {
    tracing::info!("Opening session database at {:?}", session_path);
    let session = SqliteSession::open(session_path)
        .await
        .context("Failed to open session database")?;

    tracing::info!("Creating sender pool...");
    let pool = grammers_client::sender::SenderPool::new(Arc::new(session), api_id);
    
    tracing::info!("Spawning sender pool runner...");
    tokio::spawn(pool.runner.run());
    
    let client = Client::new(pool.handle);
    
    tracing::info!("Checking authorization...");
    if !client.is_authorized().await? {
        return Err(anyhow::anyhow!(
            "Session not authorized. Please run the program first to authenticate."
        ));
    }

    Ok(client)
}

pub async fn login_and_create_client(
    api_id: i32,
    api_hash: &str,
    session_path: &std::path::Path,
) -> anyhow::Result<Client> {
    tracing::info!("Opening session database at {:?} for new login", session_path);
    let session = SqliteSession::open(session_path)
        .await
        .context("Failed to open session database")?;

    tracing::info!("Creating sender pool...");
    let pool = grammers_client::sender::SenderPool::new(Arc::new(session), api_id);
    
    tracing::info!("Spawning sender pool runner...");
    tokio::spawn(pool.runner.run());
    
    let client = Client::new(pool.handle);

    tracing::info!("Checking authorization...");
    if !client.is_authorized().await? {
        println!("Phone number login required. Starting authentication flow...");
        
        let phone_number = dialoguer::Input::<String>::new()
            .with_prompt("Enter your phone number (with country code)")
            .interact_text()?;
        
        let token = client.request_login_code(&phone_number, api_hash).await?;
        
        let code = dialoguer::Input::<String>::new()
            .with_prompt("Enter the code sent to your Telegram app")
            .interact_text()?;
        
        match client.sign_in(&token, &code).await {
            Ok(_) => {}
            Err(SignInError::PasswordRequired(password_token)) => {
                let hint = password_token.hint().unwrap_or("");
                let password = dialoguer::Password::new()
                    .with_prompt(format!("Enter your 2FA password (hint: {})", hint))
                    .interact()?;
                
                client.check_password(password_token, password).await?;
            }
            Err(SignInError::SignUpRequired) => {
                return Err(anyhow::anyhow!("This account has not signed up yet."));
            }
            Err(e) => return Err(e.into()),
        }
        
        if !client.is_authorized().await? {
            return Err(anyhow::anyhow!("Authentication failed"));
        }
        
        println!("Session saved successfully!");
    }

    Ok(client)
}
