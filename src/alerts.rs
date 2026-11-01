use crate::model::format_memory_bytes;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AlertEvent {
    ProcessCrash {
        app: String,
        worker_id: u32,
        exit_code: String,
        restarts: u32,
    },
    MemoryLimitExceeded {
        app: String,
        worker_id: u32,
        memory_bytes: u64,
        limit_bytes: u64,
    },
    ReloadSuccess {
        app: String,
        count: usize,
    },
    DeployResult {
        environment: String,
        success: bool,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertConfig {
    pub discord: Option<DiscordConfig>,
    pub slack: Option<SlackConfig>,
    pub telegram: Option<TelegramConfig>,
    pub webhook: Option<WebhookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub webhook_url: String,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    pub webhook_url: String,
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Clone)]
pub struct AlertManager {
    config: AlertConfig,
    client: Client,
}

impl AlertManager {
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub fn send(&self, event: AlertEvent) {
        let mgr = self.clone();
        tokio::spawn(async move {
            mgr.dispatch(event).await;
        });
    }

    async fn dispatch(&self, event: AlertEvent) {
        let (title, message, is_error) = match &event {
            AlertEvent::ProcessCrash {
                app,
                worker_id,
                exit_code,
                restarts,
            } => (
                format!("🚨 Gravhive Alert: Process Crash in '{}'", app),
                format!(
                    "Worker #{} crashed with exit code `{}`.\nRestart counter: {}",
                    worker_id, exit_code, restarts
                ),
                true,
            ),
            AlertEvent::MemoryLimitExceeded {
                app,
                worker_id,
                memory_bytes,
                limit_bytes,
            } => (
                format!("⚠️ Gravhive Alert: Max Memory Exceeded in '{}'", app),
                format!(
                    "Worker #{} memory usage ({} ) exceeded limit ({}). Initiating graceful reload...",
                    worker_id,
                    format_memory_bytes(*memory_bytes),
                    format_memory_bytes(*limit_bytes)
                ),
                true,
            ),
            AlertEvent::ReloadSuccess { app, count } => (
                format!("🔄 Gravhive: Zero-Downtime Reload in '{}'", app),
                format!("Successfully reloaded {} worker(s).", count),
                false,
            ),
            AlertEvent::DeployResult {
                environment,
                success,
                message,
            } => {
                if *success {
                    (
                        format!("🚀 Gravhive: Deployment Successful ({})", environment),
                        message.clone(),
                        false,
                    )
                } else {
                    (
                        format!("❌ Gravhive: Deployment Failed ({})", environment),
                        message.clone(),
                        true,
                    )
                }
            }
        };

        // 1. Discord
        if let Some(ref discord) = self.config.discord {
            let color = if is_error { 0xFF0000 } else { 0x00FF00 };
            let payload = serde_json::json!({
                "username": discord.username.as_deref().unwrap_or("Gravhive Alert"),
                "embeds": [{
                    "title": title,
                    "description": message,
                    "color": color,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }]
            });

            if let Err(e) = self
                .client
                .post(&discord.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                error!("Failed to send Discord alert: {}", e);
            } else {
                debug!("Discord alert sent successfully");
            }
        }

        // 2. Slack
        if let Some(ref slack) = self.config.slack {
            let payload = serde_json::json!({
                "text": format!("*{}*\n{}", title, message)
            });

            if let Err(e) = self
                .client
                .post(&slack.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                error!("Failed to send Slack alert: {}", e);
            } else {
                debug!("Slack alert sent successfully");
            }
        }

        // 3. Telegram
        if let Some(ref tg) = self.config.telegram {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", tg.bot_token);
            let payload = serde_json::json!({
                "chat_id": tg.chat_id,
                "text": format!("*{}*\n{}", title, message),
                "parse_mode": "Markdown"
            });

            if let Err(e) = self.client.post(&url).json(&payload).send().await {
                error!("Failed to send Telegram alert: {}", e);
            } else {
                debug!("Telegram alert sent successfully");
            }
        }

        // 4. Custom Generic Webhook
        if let Some(ref hook) = self.config.webhook {
            let mut req = self.client.post(&hook.url);
            for (k, v) in &hook.headers {
                req = req.header(k, v);
            }
            if let Err(e) = req.json(&event).send().await {
                error!("Failed to send custom webhook alert: {}", e);
            }
        }
    }
}
