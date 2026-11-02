use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Parses a string formatted like a `.env` file into a key-value HashMap.
/// Handles:
/// - Leading and trailing whitespace
/// - Comments starting with `#`
/// - Optional `export ` prefix
/// - Single and double quoted values
pub fn parse_env_str(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let clean_line = if let Some(stripped) = trimmed.strip_prefix("export ") {
            stripped.trim()
        } else {
            trimmed
        };

        if let Some((raw_key, raw_val)) = clean_line.split_once('=') {
            let key = raw_key.trim().to_string();
            if key.is_empty() {
                continue;
            }

            let mut val = raw_val.trim();

            // Handle quoted strings ("value" or 'value')
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = &val[1..val.len() - 1];
            }

            map.insert(key, val.to_string());
        }
    }

    map
}

/// Loads and parses environment variables from a `.env` file.
/// If the file extension is `.enc`, it automatically attempts to decrypt it using the GravHive vault key.
pub fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    if !path.exists() {
        anyhow::bail!("Environment file not found: {}", path.display());
    }

    let is_encrypted = path.extension().map(|ext| ext == "enc").unwrap_or(false);

    let content = if is_encrypted {
        crate::vault::SecretVault::decrypt_env_file(path)
            .with_context(|| format!("Failed to decrypt encrypted env file: {}", path.display()))?
    } else {
        fs::read_to_string(path)
            .with_context(|| format!("Failed to read env file: {}", path.display()))?
    };

    Ok(parse_env_str(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_str() {
        let content = r#"
# Database Configuration
DB_HOST=127.0.0.1
DB_PORT=5432

# Authentication
export JWT_SECRET="super-secret-key-123"
API_KEY='single-quoted-key'
EMPTY_VAL=
SPACED = with space around equals
"#;

        let parsed = parse_env_str(content);
        assert_eq!(parsed.get("DB_HOST").map(|s| s.as_str()), Some("127.0.0.1"));
        assert_eq!(parsed.get("DB_PORT").map(|s| s.as_str()), Some("5432"));
        assert_eq!(
            parsed.get("JWT_SECRET").map(|s| s.as_str()),
            Some("super-secret-key-123")
        );
        assert_eq!(
            parsed.get("API_KEY").map(|s| s.as_str()),
            Some("single-quoted-key")
        );
        assert_eq!(parsed.get("EMPTY_VAL").map(|s| s.as_str()), Some(""));
        assert_eq!(
            parsed.get("SPACED").map(|s| s.as_str()),
            Some("with space around equals")
        );
    }

    #[test]
    fn test_load_env_file_plain() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_ghive_env_{}.env", std::process::id()));
        fs::write(&path, "PORT=8080\nNODE_ENV=production\n").unwrap();

        let parsed = load_env_file(&path).unwrap();
        assert_eq!(parsed.get("PORT").map(|s| s.as_str()), Some("8080"));
        assert_eq!(
            parsed.get("NODE_ENV").map(|s| s.as_str()),
            Some("production")
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_load_env_file_encrypted() {
        let temp_dir = std::env::temp_dir();
        let plain_path = temp_dir.join(format!("test_plain_{}.env", std::process::id()));
        let enc_path = temp_dir.join(format!("test_enc_{}.env.enc", std::process::id()));

        fs::write(&plain_path, "SECRET_TOKEN=xyz123\nFEATURE_FLAG=enabled\n").unwrap();
        crate::vault::SecretVault::encrypt_env_file(&plain_path, &enc_path).unwrap();

        let parsed = load_env_file(&enc_path).unwrap();
        assert_eq!(
            parsed.get("SECRET_TOKEN").map(|s| s.as_str()),
            Some("xyz123")
        );
        assert_eq!(
            parsed.get("FEATURE_FLAG").map(|s| s.as_str()),
            Some("enabled")
        );

        let _ = fs::remove_file(plain_path);
        let _ = fs::remove_file(enc_path);
    }
}
