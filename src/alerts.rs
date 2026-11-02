use crate::common::alerts_config_path;
use crate::model::format_memory_bytes;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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
    Test {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AlertConfig {
    pub discord: Option<DiscordConfig>,
    pub slack: Option<SlackConfig>,
    pub telegram: Option<TelegramConfig>,
    pub webhook: Option<WebhookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscordConfig {
    pub webhook_url: String,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackConfig {
    pub webhook_url: String,
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl AlertConfig {
    pub fn load_global() -> Result<Option<Self>> {
        let path = alerts_config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let cfg = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
            Ok(Some(cfg))
        } else {
            Ok(None)
        }
    }

    pub fn save_global(&self) -> Result<()> {
        let path = alerts_config_path();
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write alert config to {}", path.display()))?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.discord.is_none()
            && self.slack.is_none()
            && self.telegram.is_none()
            && self.webhook.is_none()
    }

    pub fn merge(&mut self, other: AlertConfig) {
        if other.discord.is_some() {
            self.discord = other.discord;
        }
        if other.slack.is_some() {
            self.slack = other.slack;
        }
        if other.telegram.is_some() {
            self.telegram = other.telegram;
        }
        if other.webhook.is_some() {
            self.webhook = other.webhook;
        }
    }
}

pub fn mask_webhook_url(url: &str) -> String {
    if let Some((base, secret)) = url.rsplit_once('/') {
        if secret.len() > 8 {
            format!("{}/{}...[REDACTED]", base, &secret[..4])
        } else {
            format!("{}/[REDACTED]", base)
        }
    } else if url.len() > 8 {
        format!("{}...[REDACTED]", &url[..4])
    } else {
        "[REDACTED]".to_string()
    }
}

#[derive(Clone)]
pub struct AlertManager {
    config: Arc<RwLock<AlertConfig>>,
    client: Client,
}

impl AlertManager {
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            client: Client::new(),
        }
    }

    pub fn get_config(&self) -> AlertConfig {
        self.config.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn set_config(&self, new_config: AlertConfig) {
        if let Ok(mut c) = self.config.write() {
            *c = new_config;
        }
    }

    pub fn send(&self, event: AlertEvent) {
        let mgr = self.clone();
        tokio::spawn(async move {
            mgr.dispatch(event).await;
        });
    }

    pub async fn dispatch(&self, event: AlertEvent) {
        let _ = self.dispatch_and_report(event).await;
    }

    pub async fn dispatch_and_report(&self, event: AlertEvent) -> Vec<(String, bool, String)> {
        let config = self.get_config();
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
                    "Worker #{} memory usage ({}) exceeded limit ({}). Initiating graceful reload...",
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
            AlertEvent::Test { message } => (
                "🔔 Gravhive: Test Webhook Alert".to_string(),
                format!(
                    "{}\nTriggered at: {}",
                    message,
                    chrono::Utc::now().to_rfc3339()
                ),
                false,
            ),
        };

        let mut reports = Vec::new();

        // 1. Discord
        if let Some(ref discord) = config.discord {
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

            match self
                .client
                .post(&discord.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    debug!("Discord alert sent successfully");
                    reports.push((
                        "Discord".to_string(),
                        true,
                        "Alert delivered successfully".to_string(),
                    ));
                }
                Ok(resp) => {
                    let err_msg = format!("HTTP {}", resp.status());
                    error!("Failed to send Discord alert: {}", err_msg);
                    reports.push(("Discord".to_string(), false, err_msg));
                }
                Err(e) => {
                    error!("Failed to send Discord alert: {}", e);
                    reports.push(("Discord".to_string(), false, e.to_string()));
                }
            }
        }

        // 2. Slack
        if let Some(ref slack) = config.slack {
            let mut payload = serde_json::json!({
                "text": format!("*{}*\n{}", title, message)
            });
            if let Some(ref ch) = slack.channel {
                payload["channel"] = serde_json::json!(ch);
            }

            match self
                .client
                .post(&slack.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    debug!("Slack alert sent successfully");
                    reports.push((
                        "Slack".to_string(),
                        true,
                        "Alert delivered successfully".to_string(),
                    ));
                }
                Ok(resp) => {
                    let err_msg = format!("HTTP {}", resp.status());
                    error!("Failed to send Slack alert: {}", err_msg);
                    reports.push(("Slack".to_string(), false, err_msg));
                }
                Err(e) => {
                    error!("Failed to send Slack alert: {}", e);
                    reports.push(("Slack".to_string(), false, e.to_string()));
                }
            }
        }

        // 3. Telegram
        if let Some(ref tg) = config.telegram {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", tg.bot_token);
            let payload = serde_json::json!({
                "chat_id": tg.chat_id,
                "text": format!("*{}*\n{}", title, message),
                "parse_mode": "Markdown"
            });

            match self.client.post(&url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => {
                    debug!("Telegram alert sent successfully");
                    reports.push((
                        "Telegram".to_string(),
                        true,
                        "Alert delivered successfully".to_string(),
                    ));
                }
                Ok(resp) => {
                    let err_msg = format!("HTTP {}", resp.status());
                    error!("Failed to send Telegram alert: {}", err_msg);
                    reports.push(("Telegram".to_string(), false, err_msg));
                }
                Err(e) => {
                    error!("Failed to send Telegram alert: {}", e);
                    reports.push(("Telegram".to_string(), false, e.to_string()));
                }
            }
        }

        // 4. Custom Generic Webhook
        if let Some(ref hook) = config.webhook {
            let mut req = self.client.post(&hook.url);
            for (k, v) in &hook.headers {
                req = req.header(k, v);
            }
            match req.json(&event).send().await {
                Ok(resp) if resp.status().is_success() => {
                    debug!("Custom webhook alert sent successfully");
                    reports.push((
                        "Generic Webhook".to_string(),
                        true,
                        format!("Alert delivered successfully ({})", resp.status()),
                    ));
                }
                Ok(resp) => {
                    let err_msg = format!("HTTP {}", resp.status());
                    error!("Failed to send custom webhook alert: {}", err_msg);
                    reports.push(("Generic Webhook".to_string(), false, err_msg));
                }
                Err(e) => {
                    error!("Failed to send custom webhook alert: {}", e);
                    reports.push(("Generic Webhook".to_string(), false, e.to_string()));
                }
            }
        }

        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_webhook_url() {
        assert_eq!(
            mask_webhook_url("https://discord.com/api/webhooks/12345/abcdefghijk"),
            "https://discord.com/api/webhooks/12345/abcd...[REDACTED]"
        );
        assert_eq!(
            mask_webhook_url("https://example.com/short"),
            "https://example.com/[REDACTED]"
        );
    }

    #[test]
    fn test_alert_config_merge() {
        let mut cfg1 = AlertConfig {
            discord: Some(DiscordConfig {
                webhook_url: "https://discord.com/1".into(),
                username: None,
            }),
            slack: None,
            telegram: None,
            webhook: None,
        };

        let cfg2 = AlertConfig {
            discord: None,
            slack: Some(SlackConfig {
                webhook_url: "https://slack.com/2".into(),
                channel: Some("#general".into()),
            }),
            telegram: None,
            webhook: None,
        };

        cfg1.merge(cfg2);
        assert!(cfg1.discord.is_some());
        assert!(cfg1.slack.is_some());
        assert_eq!(cfg1.slack.unwrap().channel.as_deref(), Some("#general"));
    }

    #[test]
    fn test_alert_config_is_empty() {
        let empty_cfg = AlertConfig::default();
        assert!(empty_cfg.is_empty());

        let non_empty = AlertConfig {
            discord: Some(DiscordConfig {
                webhook_url: "https://discord.com/xyz".into(),
                username: None,
            }),
            slack: None,
            telegram: None,
            webhook: None,
        };
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_alert_manager_set_config() {
        let mgr = AlertManager::new(AlertConfig::default());
        assert!(mgr.get_config().is_empty());

        let new_cfg = AlertConfig {
            discord: Some(DiscordConfig {
                webhook_url: "https://discord.com/hook".into(),
                username: Some("Bot".into()),
            }),
            slack: None,
            telegram: None,
            webhook: None,
        };
        mgr.set_config(new_cfg.clone());
        assert_eq!(mgr.get_config(), new_cfg);
    }
}
