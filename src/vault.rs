use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_VAULT_FILE: &str = "vault.enc";
const NONCE_LEN: usize = 12;

static IN_MEMORY_KEY_CACHE: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaultData {
    pub secrets: HashMap<String, String>,
}

pub struct SecretVault;

impl SecretVault {
    pub fn get_master_key() -> Result<[u8; 32]> {
        if let Ok(key_str) =
            std::env::var("GHIVE_MASTER_KEY").or_else(|_| std::env::var("GRAVHIVE_MASTER_KEY"))
        {
            let trimmed = key_str.trim();
            if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                let mut key = [0u8; 32];
                for i in 0..32 {
                    key[i] = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16).unwrap_or(0);
                }
                return Ok(key);
            }
            // Cryptographically derive 32-byte key from arbitrary passphrase using SHA-256
            let mut hasher = Sha256::new();
            hasher.update(trimmed.as_bytes());
            let hash_output = hasher.finalize();
            let mut key = [0u8; 32];
            key.copy_from_slice(&hash_output);
            return Ok(key);
        }

        // Check keyfile in ~/.gravhive/master.key
        let key_file = crate::common::gravhive_dir().join("master.key");
        if key_file.exists() {
            let bytes = fs::read(&key_file)?;
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(key);
            } else if bytes.len() == 64 {
                if let Ok(hex_str) = std::str::from_utf8(&bytes) {
                    if hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
                        let mut key = [0u8; 32];
                        for i in 0..32 {
                            key[i] =
                                u8::from_str_radix(&hex_str[i * 2..i * 2 + 2], 16).unwrap_or(0);
                        }
                        return Ok(key);
                    }
                }
            }
        }

        // If already cached in memory during this process, reuse it
        if let Some(cached) = IN_MEMORY_KEY_CACHE.get() {
            return Ok(*cached);
        }

        // Ensure ~/.gravhive parent directory exists before creating master.key
        if let Some(parent) = key_file.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Generate and save 32-byte cryptographically secure master key (CSPRNG)
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        let _ = fs::write(&key_file, key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&key_file, fs::Permissions::from_mode(0o600));
        }
        let _ = IN_MEMORY_KEY_CACHE.set(key);
        Ok(key)
    }

    pub fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| anyhow::anyhow!("Invalid AES-GCM key: {}", e))?;

        // Generate cryptographically secure nonce (12 bytes) using CSPRNG
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        // Prepend nonce to ciphertext
        let mut result = nonce_bytes.to_vec();
        result.extend(ciphertext);
        Ok(result)
    }

    pub fn decrypt(ciphertext_with_nonce: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
        if ciphertext_with_nonce.len() < NONCE_LEN {
            bail!("Ciphertext is too short (corrupted payload)");
        }

        let (nonce_bytes, ciphertext) = ciphertext_with_nonce.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| anyhow::anyhow!("Invalid AES-GCM key: {}", e))?;
        let nonce_array: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid nonce length"))?;
        let nonce = Nonce::from(nonce_array);

        let plaintext = cipher.decrypt(&nonce, ciphertext).map_err(|e| {
            anyhow::anyhow!("Decryption failed (incorrect key or corrupted data): {}", e)
        })?;

        Ok(plaintext)
    }

    pub fn vault_file_path() -> PathBuf {
        crate::common::gravhive_dir().join(DEFAULT_VAULT_FILE)
    }

    pub fn load_vault() -> Result<VaultData> {
        let path = Self::vault_file_path();
        if !path.exists() {
            return Ok(VaultData::default());
        }

        let key = Self::get_master_key()?;
        let encrypted = fs::read(&path)?;
        let decrypted = Self::decrypt(&encrypted, &key)?;
        let data: VaultData =
            serde_json::from_slice(&decrypted).context("Failed to deserialize vault data")?;
        Ok(data)
    }

    pub fn save_vault(data: &VaultData) -> Result<()> {
        let path = Self::vault_file_path();
        let key = Self::get_master_key()?;
        let json = serde_json::to_vec(data)?;
        let encrypted = Self::encrypt(&json, &key)?;
        fs::write(&path, encrypted)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn set_secret(key: &str, value: &str) -> Result<()> {
        let mut vault = Self::load_vault()?;
        vault.secrets.insert(key.to_string(), value.to_string());
        Self::save_vault(&vault)?;
        Ok(())
    }

    pub fn get_secret(key: &str) -> Result<Option<String>> {
        let vault = Self::load_vault()?;
        Ok(vault.secrets.get(key).cloned())
    }

    pub fn list_secrets() -> Result<Vec<(String, String)>> {
        let vault = Self::load_vault()?;
        let mut list = Vec::new();
        for (k, v) in vault.secrets {
            let masked = mask_secret_value(&v);
            list.push((k, masked));
        }
        list.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(list)
    }

    pub fn encrypt_env_file(input_path: &Path, output_path: &Path) -> Result<()> {
        let content = fs::read(input_path)
            .with_context(|| format!("Failed to read env file: {}", input_path.display()))?;
        let key = Self::get_master_key()?;
        let encrypted = Self::encrypt(&content, &key)?;
        fs::write(output_path, encrypted)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(output_path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn decrypt_env_file(input_path: &Path) -> Result<String> {
        let encrypted = fs::read(input_path)
            .with_context(|| format!("Failed to read encrypted file: {}", input_path.display()))?;
        let key = Self::get_master_key()?;
        let decrypted = Self::decrypt(&encrypted, &key)?;
        let text = String::from_utf8(decrypted)?;
        Ok(text)
    }

    /// Resolves a value that might reference a vault secret via the `vault:<SECRET_NAME>` syntax.
    /// If the value does not start with `vault:`, it is returned unchanged.
    pub fn resolve_secret_value(val: &str) -> Result<String> {
        if let Some(key_name) = val.strip_prefix("vault:") {
            let key_trimmed = key_name.trim();
            match Self::get_secret(key_trimmed)? {
                Some(secret) => Ok(secret),
                None => bail!(
                    "Secret key '{}' referenced as 'vault:{}' was not found in vault",
                    key_trimmed,
                    key_trimmed
                ),
            }
        } else {
            Ok(val.to_string())
        }
    }
}

pub fn mask_env_map(env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut masked = HashMap::new();
    for (k, v) in env {
        let is_sensitive_key = k.to_uppercase().contains("PASS")
            || k.to_uppercase().contains("SECRET")
            || k.to_uppercase().contains("KEY")
            || k.to_uppercase().contains("TOKEN")
            || k.to_uppercase().contains("AUTH");
        if v.starts_with("vault:") {
            masked.insert(k.clone(), "[VAULT:***]".to_string());
        } else if is_sensitive_key {
            masked.insert(k.clone(), mask_secret_value(v));
        } else {
            masked.insert(k.clone(), v.clone());
        }
    }
    masked
}

pub fn mask_secret_value(val: &str) -> String {
    if val.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}****", &val[..2])
    }
}

pub fn mask_sensitive_text(text: &str, secret_values: &[String]) -> String {
    let mut result = text.to_string();
    for secret in secret_values {
        if !secret.trim().is_empty() && secret.len() >= 3 {
            result = result.replace(secret, "[REDACTED]");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_aes_gcm() {
        let key = [42u8; 32];
        let original_data = b"DATABASE_URL=postgres://root:password123@localhost:5432/app";
        let encrypted = SecretVault::encrypt(original_data, &key).expect("Encryption failed");
        assert_ne!(encrypted.as_slice(), original_data);

        let decrypted = SecretVault::decrypt(&encrypted, &key).expect("Decryption failed");
        assert_eq!(decrypted, original_data);
    }

    #[test]
    fn test_csprng_entropy_and_uniqueness() {
        let key = [99u8; 32];
        let data = b"Sensitive Payload 12345";
        let mut nonces = std::collections::HashSet::new();

        for _ in 0..100 {
            let encrypted = SecretVault::encrypt(data, &key).expect("Encryption failed");
            assert!(encrypted.len() > NONCE_LEN);
            let nonce = encrypted[..NONCE_LEN].to_vec();
            assert!(nonces.insert(nonce), "CSPRNG produced duplicate nonce!");
        }
    }

    #[test]
    fn test_mask_secret_value() {
        assert_eq!(mask_secret_value("abc"), "****");
        assert_eq!(mask_secret_value("my_super_secret"), "my****");
    }

    #[test]
    fn test_mask_sensitive_text() {
        let text =
            "Connected to postgres://root:super_secret_pw@10.0.0.1:5432 with token tok_123456789";
        let secrets = vec!["super_secret_pw".to_string(), "tok_123456789".to_string()];
        let redacted = mask_sensitive_text(text, &secrets);
        assert!(!redacted.contains("super_secret_pw"));
        assert!(!redacted.contains("tok_123456789"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_passphrase_key_derivation() {
        std::env::set_var("GHIVE_MASTER_KEY", "my-secure-production-passphrase-2026");
        let key1 = SecretVault::get_master_key().expect("Failed to derive master key");
        assert_ne!(key1, [0u8; 32]);

        // Key must be deterministic
        let key2 = SecretVault::get_master_key().expect("Failed to derive master key");
        assert_eq!(key1, key2);

        // Different passphrase produces completely different key
        std::env::set_var("GHIVE_MASTER_KEY", "another-passphrase");
        let key3 = SecretVault::get_master_key().expect("Failed to derive master key");
        assert_ne!(key1, key3);

        std::env::remove_var("GHIVE_MASTER_KEY");
    }
}
