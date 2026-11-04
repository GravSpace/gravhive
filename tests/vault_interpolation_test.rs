use gravhive::env_loader::resolve_env_vault_references;
use gravhive::vault::{mask_env_map, SecretVault};
use std::collections::HashMap;

#[test]
fn test_vault_secret_resolution_and_masking() {
    // Set a custom master key for test isolation
    let custom_home = std::env::temp_dir().join(format!("test_ghive_vault_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&custom_home);
    std::env::set_var("GHIVE_HOME", &custom_home);
    std::env::set_var(
        "GHIVE_MASTER_KEY",
        "test-passphrase-for-vault-interpolation",
    );

    // Store a test secret
    SecretVault::set_secret("PROD_DB_PASS", "super_strong_password_999").unwrap();
    SecretVault::set_secret("STRIPE_KEY", "sk_live_1234567890abcdef").unwrap();

    // Verify resolve_secret_value directly
    let resolved = SecretVault::resolve_secret_value("vault:PROD_DB_PASS").unwrap();
    assert_eq!(resolved, "super_strong_password_999");

    let plain = SecretVault::resolve_secret_value("plain_value_without_prefix").unwrap();
    assert_eq!(plain, "plain_value_without_prefix");

    // Verify error for missing secret
    let err = SecretVault::resolve_secret_value("vault:NON_EXISTENT_KEY");
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("not found in vault"));

    // Verify resolve_env_vault_references on a HashMap
    let mut env_map = HashMap::new();
    env_map.insert(
        "DATABASE_URL".to_string(),
        "postgres://user:pass@localhost:5432".to_string(),
    );
    env_map.insert("DB_PASS".to_string(), "vault:PROD_DB_PASS".to_string());
    env_map.insert("API_TOKEN".to_string(), "vault:STRIPE_KEY".to_string());

    // Before resolution, verify mask_env_map
    let masked_map = mask_env_map(&env_map);
    assert_eq!(
        masked_map.get("DB_PASS").map(|s| s.as_str()),
        Some("[VAULT:***]")
    );
    assert_eq!(
        masked_map.get("API_TOKEN").map(|s| s.as_str()),
        Some("[VAULT:***]")
    );
    assert_eq!(
        masked_map.get("DATABASE_URL").map(|s| s.as_str()),
        Some("postgres://user:pass@localhost:5432")
    );

    // Now resolve
    resolve_env_vault_references(&mut env_map).unwrap();
    assert_eq!(
        env_map.get("DB_PASS").map(|s| s.as_str()),
        Some("super_strong_password_999")
    );
    assert_eq!(
        env_map.get("API_TOKEN").map(|s| s.as_str()),
        Some("sk_live_1234567890abcdef")
    );
    assert_eq!(
        env_map.get("DATABASE_URL").map(|s| s.as_str()),
        Some("postgres://user:pass@localhost:5432")
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&custom_home);
    std::env::remove_var("GHIVE_HOME");
    std::env::remove_var("GHIVE_MASTER_KEY");
}
