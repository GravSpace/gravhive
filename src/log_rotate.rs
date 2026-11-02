use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::Path;
use tracing::{debug, error};

/// Checks if a log file exceeds `max_bytes`, and if so, rotates it.
/// Rotated files follow the pattern `name.log.1(.gz)`, `name.log.2(.gz)`, up to `keep`.
pub fn check_and_rotate_file(
    file_path: &Path,
    max_bytes: u64,
    keep: u32,
    compress: bool,
) -> Result<bool> {
    if !file_path.exists() {
        return Ok(false);
    }

    let meta = fs::metadata(file_path)?;
    if meta.len() < max_bytes {
        return Ok(false);
    }

    rotate_file_now(file_path, keep, compress)
}

/// Rotates a log file immediately regardless of size.
pub fn rotate_file_now(file_path: &Path, keep: u32, compress: bool) -> Result<bool> {
    if !file_path.exists() {
        return Ok(false);
    }

    let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return Ok(false),
    };

    let keep = keep.max(1);

    // 1. Shift older archives from keep down to 1
    for i in (1..=keep).rev() {
        let current_gz = parent.join(format!("{}.{}.gz", file_name, i));
        let current_plain = parent.join(format!("{}.{}", file_name, i));

        if i == keep {
            let _ = fs::remove_file(&current_gz);
            let _ = fs::remove_file(&current_plain);
        } else {
            let next_gz = parent.join(format!("{}.{}.gz", file_name, i + 1));
            let next_plain = parent.join(format!("{}.{}", file_name, i + 1));

            if current_gz.exists() {
                let _ = fs::rename(&current_gz, &next_gz);
            }
            if current_plain.exists() {
                let _ = fs::rename(&current_plain, &next_plain);
            }
        }
    }

    // 2. Move current active log to .1
    let rotated_first = parent.join(format!("{}.1", file_name));
    fs::rename(file_path, &rotated_first)
        .with_context(|| format!("Failed to rotate log file {:?}", file_path))?;

    // 3. Compress to .1.gz if requested
    if compress {
        let gz_target = parent.join(format!("{}.1.gz", file_name));
        tokio::spawn(async move {
            if let Err(e) = compress_file_gzip(&rotated_first, &gz_target) {
                error!(
                    "Failed to compress rotated log file {:?}: {}",
                    rotated_first, e
                );
            } else {
                debug!("Compressed rotated log file to {:?}", gz_target);
            }
        });
    }

    Ok(true)
}

/// Compresses a file with gzip and deletes the original file upon success.
pub fn compress_file_gzip(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    let input = File::open(src).with_context(|| format!("Failed to open {:?}", src))?;
    let mut reader = BufReader::new(input);

    let output = File::create(dst).with_context(|| format!("Failed to create {:?}", dst))?;
    let writer = BufWriter::new(output);
    let mut encoder = GzEncoder::new(writer, Compression::default());

    io::copy(&mut reader, &mut encoder)
        .with_context(|| format!("Failed to compress {:?} into {:?}", src, dst))?;
    encoder.finish()?;

    // Remove uncompressed source after successful compression
    let _ = fs::remove_file(src);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[test]
    fn test_log_rotation_and_gzip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "ghive_test_logrotate_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp_dir);
        let log_file = temp_dir.join("test-app-out.log");

        // Write 100 bytes
        let data = "A".repeat(100);
        fs::write(&log_file, &data).unwrap();

        // Rotate when threshold is 50 bytes
        let rotated = check_and_rotate_file(&log_file, 50, 3, false).unwrap();
        assert!(rotated);
        assert!(!log_file.exists());
        let rotated_1 = temp_dir.join("test-app-out.log.1");
        assert!(rotated_1.exists());

        // Test manual compress
        let gz_file = temp_dir.join("test-app-out.log.1.gz");
        compress_file_gzip(&rotated_1, &gz_file).unwrap();
        assert!(!rotated_1.exists());
        assert!(gz_file.exists());

        // Verify decompressed content
        let gz_in = File::open(&gz_file).unwrap();
        let mut decoder = GzDecoder::new(gz_in);
        let mut decompressed = String::new();
        decoder.read_to_string(&mut decompressed).unwrap();
        assert_eq!(decompressed, data);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
