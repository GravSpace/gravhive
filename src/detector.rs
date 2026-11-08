use std::path::Path;

#[derive(Debug, Clone)]
pub struct DetectedExecution {
    pub interpreter: Option<String>,
    pub script: String,
    pub args: Vec<String>,
}

pub fn detect_runtime(input: &str, initial_args: &[String]) -> DetectedExecution {
    let trimmed = input.trim();

    // Check if input is a compound command string like "php -S 0.0.0.0:8000 index.php"
    if trimmed.contains(' ') && !Path::new(trimmed).exists() {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if !parts.is_empty() {
            let command = parts[0].to_string();
            let mut args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            args.extend(initial_args.to_vec());

            return DetectedExecution {
                interpreter: None,
                script: command,
                args,
            };
        }
    }

    let path = Path::new(trimmed);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut final_args = initial_args.to_vec();

    match ext.as_str() {
        "js" | "mjs" | "cjs" => {
            // Prefer node if available, fallback to bun or deno
            let interp = if is_command_available("node") {
                "node"
            } else if is_command_available("bun") {
                "bun"
            } else if is_command_available("deno") {
                final_args.insert(0, "run".to_string());
                final_args.insert(1, "-A".to_string());
                "deno"
            } else {
                "node"
            };
            DetectedExecution {
                interpreter: Some(interp.to_string()),
                script: trimmed.to_string(),
                args: final_args,
            }
        }
        "ts" | "mts" | "cts" => {
            // Prefer bun if bun is available in PATH, otherwise deno or ts-node
            let interp = if is_command_available("bun") {
                "bun"
            } else if is_command_available("deno") {
                final_args.insert(0, "run".to_string());
                final_args.insert(1, "-A".to_string());
                "deno"
            } else {
                "node"
            };
            DetectedExecution {
                interpreter: Some(interp.to_string()),
                script: trimmed.to_string(),
                args: final_args,
            }
        }
        "py" => DetectedExecution {
            interpreter: Some("python3".to_string()),
            script: trimmed.to_string(),
            args: final_args,
        },
        "php" => DetectedExecution {
            interpreter: Some("php".to_string()),
            script: trimmed.to_string(),
            args: final_args,
        },
        "sh" | "bash" => DetectedExecution {
            interpreter: Some("/bin/bash".to_string()),
            script: trimmed.to_string(),
            args: final_args,
        },
        _ => {
            // Native binary or direct command
            DetectedExecution {
                interpreter: None,
                script: trimmed.to_string(),
                args: final_args,
            }
        }
    }
}

fn is_command_available(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let p = dir.join(cmd);
            if p.is_file() {
                return true;
            }
        }
    }
    false
}
