use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedExecution {
    pub interpreter: Option<String>,
    pub script: String,
    pub args: Vec<String>,
}

pub fn get_shell() -> String {
    if Path::new("/bin/bash").exists() {
        "/bin/bash".to_string()
    } else if Path::new("/bin/sh").exists() {
        "/bin/sh".to_string()
    } else {
        "sh".to_string()
    }
}

pub fn contains_shell_syntax(input: &str, args: &[String]) -> bool {
    let trimmed = input.trim();

    // If the input is a single file that exists on disk, it's not a compound shell string
    if !trimmed.contains(' ') && Path::new(trimmed).exists() {
        return false;
    }

    // Check if input starts with shell invocation
    if trimmed.starts_with("sh -c")
        || trimmed.starts_with("/bin/sh -c")
        || trimmed.starts_with("bash -c")
        || trimmed.starts_with("/bin/bash -c")
    {
        return true;
    }

    // Shell control operators and metacharacters
    let operators = ["&&", "||", ";", "|", ">", "<", "$(", "`"];

    for op in &operators {
        if input.contains(op) {
            return true;
        }
    }

    // Check for standalone & (background) or & followed by space / end of string
    if trimmed.ends_with('&') || trimmed.contains("& ") || trimmed.contains(" &") {
        return true;
    }

    // Check args
    for arg in args {
        for op in &operators {
            if arg.contains(op) {
                return true;
            }
        }
        if arg == "&" {
            return true;
        }
    }

    false
}

pub fn clean_shell_prefix(cmd: &str) -> String {
    let trimmed = cmd.trim();
    let stripped = if let Some(rest) = trimmed.strip_prefix("/bin/bash -c ") {
        rest.trim()
    } else if let Some(rest) = trimmed.strip_prefix("bash -c ") {
        rest.trim()
    } else if let Some(rest) = trimmed.strip_prefix("/bin/sh -c ") {
        rest.trim()
    } else if let Some(rest) = trimmed.strip_prefix("sh -c ") {
        rest.trim()
    } else {
        trimmed
    };

    if (stripped.starts_with('\'') && stripped.ends_with('\'') && stripped.len() >= 2)
        || (stripped.starts_with('"') && stripped.ends_with('"') && stripped.len() >= 2)
    {
        stripped[1..stripped.len() - 1].trim().to_string()
    } else {
        stripped.to_string()
    }
}

pub fn detect_runtime(input: &str, initial_args: &[String]) -> DetectedExecution {
    let trimmed = input.trim();

    // Check if input is already a shell invocation with -c
    if (trimmed == "sh" || trimmed == "/bin/sh" || trimmed == "bash" || trimmed == "/bin/bash")
        && initial_args.first().map(|s| s.as_str()) == Some("-c")
    {
        let full_cmd = initial_args[1..].join(" ");
        let clean_cmd = clean_shell_prefix(&full_cmd);
        return DetectedExecution {
            interpreter: Some(trimmed.to_string()),
            script: "-c".to_string(),
            args: vec![clean_cmd],
        };
    }

    // Check if input or args contains shell syntax (e.g. &&, ||, ;, |, >, <, etc.)
    if contains_shell_syntax(trimmed, initial_args) {
        let shell = get_shell();
        let full_cmd = if trimmed == "-c" {
            initial_args.join(" ")
        } else if initial_args.is_empty() {
            trimmed.to_string()
        } else {
            format!("{} {}", trimmed, initial_args.join(" "))
                .trim()
                .to_string()
        };

        let clean_cmd = clean_shell_prefix(&full_cmd);

        return DetectedExecution {
            interpreter: Some(shell),
            script: "-c".to_string(),
            args: vec![clean_cmd],
        };
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_operator_detection() {
        let cmd = "docker restart surrealdb && /usr/bin/pm2 restart 7 8 11";
        assert!(contains_shell_syntax(cmd, &[]));

        let detected = detect_runtime(cmd, &[]);
        assert!(detected.interpreter.is_some());
        assert_eq!(detected.script, "-c");
        assert_eq!(detected.args, vec![cmd.to_string()]);
    }

    #[test]
    fn test_shell_operator_in_args() {
        let cmd = "docker";
        let args = vec![
            "restart".to_string(),
            "surrealdb".to_string(),
            "&&".to_string(),
            "/usr/bin/pm2".to_string(),
            "restart".to_string(),
            "7".to_string(),
            "8".to_string(),
            "11".to_string(),
        ];
        assert!(contains_shell_syntax(cmd, &args));

        let detected = detect_runtime(cmd, &args);
        assert!(detected.interpreter.is_some());
        assert_eq!(detected.script, "-c");
        assert_eq!(
            detected.args,
            vec!["docker restart surrealdb && /usr/bin/pm2 restart 7 8 11".to_string()]
        );
    }

    #[test]
    fn test_pipe_and_redirection_detection() {
        assert!(contains_shell_syntax("echo hello | grep h", &[]));
        assert!(contains_shell_syntax("echo hello > /tmp/out.txt", &[]));
        assert!(contains_shell_syntax("cat < /tmp/in.txt", &[]));
        assert!(contains_shell_syntax("docker ps || echo failed", &[]));
        assert!(contains_shell_syntax("cmd1; cmd2", &[]));
    }

    #[test]
    fn test_clean_shell_prefix() {
        assert_eq!(
            clean_shell_prefix("sh -c 'echo 1 && echo 2'"),
            "echo 1 && echo 2"
        );
        assert_eq!(
            clean_shell_prefix("bash -c \"echo 1 && echo 2\""),
            "echo 1 && echo 2"
        );
        assert_eq!(
            clean_shell_prefix("/bin/bash -c 'docker restart app'"),
            "docker restart app"
        );
    }

    #[test]
    fn test_direct_compound_command_not_shell() {
        let cmd = "php -S 0.0.0.0:8000 index.php";
        assert!(!contains_shell_syntax(cmd, &[]));

        let detected = detect_runtime(cmd, &[]);
        assert_eq!(detected.interpreter, None);
        assert_eq!(detected.script, "php");
        assert_eq!(detected.args, vec!["-S", "0.0.0.0:8000", "index.php"]);
    }
}
