use gravhive::alerts::{mask_webhook_url, AlertConfig, DiscordConfig, SlackConfig, TelegramConfig, WebhookConfig};
use std::collections::HashMap;

#[test]
fn test_mask_webhook_urls() {
    assert_eq!(
        mask_webhook_url("https://discord.com/api/webhooks/12345/abcdefghijk"),
        "https://discord.com/api/webhooks/12345/abcd...[REDACTED]"
    );
    assert_eq!(
        mask_webhook_url("https://hooks.slack.com/services/T00/B00/X12345678"),
        "https://hooks.slack.com/services/T00/B00/X123...[REDACTED]"
    );
    assert_eq!(
        mask_webhook_url("short"),
        "[REDACTED]"
    );
}

#[test]
fn test_alert_config_serialization_cycle() {
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer token123".to_string());

    let config = AlertConfig {
        discord: Some(DiscordConfig {
            webhook_url: "https://discord.com/api/webhooks/123/xyz".to_string(),
            username: Some("Gravhive Bot".to_string()),
        }),
        slack: Some(SlackConfig {
            webhook_url: "https://hooks.slack.com/services/T0/B0/X0".to_string(),
            channel: Some("#prod-alerts".to_string()),
        }),
        telegram: Some(TelegramConfig {
            bot_token: "123456:ABC-DEF".to_string(),
            chat_id: "987654321".to_string(),
        }),
        webhook: Some(WebhookConfig {
            url: "https://example.com/alerts".to_string(),
            headers,
        }),
    };

    let json = serde_json::to_string_pretty(&config).expect("Serialize failed");
    let deserialized: AlertConfig = serde_json::from_str(&json).expect("Deserialize failed");

    assert_eq!(config, deserialized);
}

