use std::path::{Path, PathBuf};

/// Resolves a path lexically to an absolute path without dereferencing symlinks.
///
/// Bare command names (e.g. `docker`, `node`) without path separators that do not
/// exist in `base_dir` (or the current directory) are returned unchanged for `$PATH` lookup.
pub fn resolve_lexical_path(raw: &str, base_dir: Option<&Path>) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let raw_path = Path::new(trimmed);
    let has_sep = trimmed.contains('/') || trimmed.contains('\\');

    let effective_base = resolve_base_dir(base_dir);

    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else if has_sep {
        effective_base.join(raw_path)
    } else {
        let joined = effective_base.join(raw_path);
        if joined.exists() {
            joined
        } else {
            return trimmed.to_string();
        }
    };

    std::path::absolute(&candidate)
        .unwrap_or(candidate)
        .to_string_lossy()
        .into_owned()
}

/// Resolves an optional directory path lexically to an absolute `PathBuf`,
/// falling back to the current working directory if `dir` is `None` or empty.
pub fn resolve_cwd(dir: Option<&Path>) -> Option<PathBuf> {
    match dir {
        Some(p) if !p.as_os_str().is_empty() => {
            if p.is_absolute() {
                std::path::absolute(p).ok().or_else(|| Some(p.to_path_buf()))
            } else {
                let base = std::env::current_dir().ok()?;
                let joined = base.join(p);
                std::path::absolute(&joined).ok().or(Some(joined))
            }
        }
        _ => std::env::current_dir().ok(),
    }
}

fn resolve_base_dir(base_dir: Option<&Path>) -> PathBuf {
    match base_dir {
        Some(p) if !p.as_os_str().is_empty() => {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(p))
                    .unwrap_or_else(|_| p.to_path_buf())
            }
        }
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_lexical_path_preserves_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("ghive_lex_test_{}", rand::random::<u64>()));
        let v1_dir = root.join("releases/v1");
        let v2_dir = root.join("releases/v2");
        std::fs::create_dir_all(&v1_dir).unwrap();
        std::fs::create_dir_all(&v2_dir).unwrap();

        let v1_bin = v1_dir.join("app");
        let v2_bin = v2_dir.join("app");
        std::fs::write(&v1_bin, "v1").unwrap();
        std::fs::write(&v2_bin, "v2").unwrap();

        let current_link = root.join("current");
        symlink(&v1_dir, &current_link).unwrap();

        let target_via_link = current_link.join("app");
        let resolved = resolve_lexical_path(target_via_link.to_str().unwrap(), None);

        // Must preserve symlink path (`.../current/app`), NOT dereference to `releases/v1/app`
        assert_eq!(resolved, target_via_link.to_string_lossy());

        // Swap symlink to v2 and confirm resolved path now points to v2 content
        std::fs::remove_file(&current_link).unwrap();
        symlink(&v2_dir, &current_link).unwrap();
        assert_eq!(std::fs::read_to_string(&resolved).unwrap(), "v2");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_lexical_path_keeps_bare_path_command() {
        assert_eq!(
            resolve_lexical_path("nonexistent_cmd_xyz", None),
            "nonexistent_cmd_xyz"
        );
    }
}
