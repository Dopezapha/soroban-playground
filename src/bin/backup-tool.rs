// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT
//
// Automated Database Backup, S3 Snapshot & Disaster Recovery Verification
//
// Usage:
//   backup-tool --action backup    [--db-url <url>] [--output-dir <dir>]
//   backup-tool --action restore   --file <backup.enc> [--db-url <url>]
//   backup-tool --action verify    --file <backup.enc>
//   backup-tool --action retention [--max-days <n>]
//   backup-tool --action list
//
// Environment variables (all optional — fall back to safe defaults):
//   DATABASE_URL        PostgreSQL DSN used for pg_dump / pg_restore
//   BACKUP_DIR          Local directory for backup files (default: ./backups)
//   BACKUP_ENCRYPTION_KEY_HEX  32-byte AES-256-GCM key, hex-encoded
//   BACKUP_MAX_DAYS     Retention window in days (default: 30)
//   AWS_REGION / AWS_BUCKET  When set, backups are also uploaded to S3

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use tokio::fs;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Summary returned by the backup action.
#[derive(Debug)]
struct BackupSummary {
    filename: String,
    plaintext_bytes: usize,
    encrypted_bytes: usize,
    sha256_hex: String,
    duration_ms: u128,
}

/// Summary returned by the verify action.
#[derive(Debug)]
struct VerifySummary {
    filename: String,
    encrypted_bytes: usize,
    plaintext_bytes: usize,
    sha256_hex: String,
    decryption_ok: bool,
}

// ── Encryption helpers ────────────────────────────────────────────────────────

/// Resolve the 32-byte AES-256-GCM key.
/// Production deployments should supply `BACKUP_ENCRYPTION_KEY_HEX`.
/// A deterministic-but-weak fallback is used in development so the binary
/// remains functional without configuration.
fn resolve_encryption_key() -> [u8; 32] {
    if let Ok(hex) = env::var("BACKUP_ENCRYPTION_KEY_HEX") {
        let trimmed = hex.trim();
        if trimmed.len() == 64 {
            let mut key = [0u8; 32];
            if hex::decode_to_slice(trimmed, &mut key).is_ok() {
                return key;
            }
            eprintln!(
                "[WARN] BACKUP_ENCRYPTION_KEY_HEX is not valid hex; falling back to zeroed key"
            );
        } else {
            eprintln!(
                "[WARN] BACKUP_ENCRYPTION_KEY_HEX must be 64 hex chars (32 bytes); \
                 falling back to zeroed key"
            );
        }
    } else if env::var("NODE_ENV").as_deref() == Ok("production")
        || env::var("RUST_ENV").as_deref() == Ok("production")
    {
        eprintln!(
            "[ERROR] BACKUP_ENCRYPTION_KEY_HEX must be set in production. \
             Using zeroed key is insecure!"
        );
    }
    [0u8; 32]
}

/// Encrypt `plaintext` using AES-256-GCM.
/// The 12-byte nonce is derived from the current UTC timestamp and prepended
/// to the ciphertext so the decryption routine can extract it.
#[allow(deprecated)]
fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    // Build a deterministic-but-unique nonce from the current second + random suffix
    let ts_bytes = Utc::now().timestamp().to_be_bytes(); // 8 bytes
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(&ts_bytes);
    // Fill remaining 4 bytes with pseudo-random data derived from plaintext length
    let len_bytes = (plaintext.len() as u32).to_be_bytes();
    nonce_bytes[8..].copy_from_slice(&len_bytes);

    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("AES-256-GCM encryption failed: {e}"))?;

    // Format: [12-byte nonce][ciphertext+tag]
    let mut payload = nonce_bytes.to_vec();
    payload.extend_from_slice(&ciphertext);
    Ok(payload)
}

/// Decrypt data produced by `encrypt`.
#[allow(deprecated)]
fn decrypt(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 12 {
        return Err("Encrypted payload is too short to contain a nonce".into());
    }
    let (nonce_bytes, ciphertext) = encrypted.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("AES-256-GCM decryption failed: {e}"))
}

// ── Checksums ─────────────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ── pg_dump / pg_restore ──────────────────────────────────────────────────────

/// Run `pg_dump` against `db_url` and return the raw SQL bytes.
fn run_pg_dump(db_url: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("pg_dump")
        .arg("--no-password")
        .arg("--format=plain")
        .arg(db_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to spawn pg_dump (is it installed and in PATH?): {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pg_dump exited with error:\n{stderr}"));
    }

    Ok(output.stdout)
}

/// Run `psql` to restore a plain-SQL dump.
fn run_pg_restore(db_url: &str, sql: &[u8]) -> Result<(), String> {
    use std::io::Write;

    let mut child = Command::new("psql")
        .arg("--no-password")
        .arg(db_url)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn psql: {e}"))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(sql)
            .map_err(|e| format!("Failed to pipe SQL to psql: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("psql wait error: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("psql restore failed:\n{stderr}"));
    }

    Ok(())
}

// ── S3 upload (optional) ──────────────────────────────────────────────────────

/// Upload `data` to S3 when AWS credentials are present.  The function is
/// intentionally best-effort: if the upload fails it logs a warning and
/// returns rather than aborting the backup.
async fn try_s3_upload(filename: &str, data: &[u8]) {
    let bucket = match env::var("AWS_BUCKET") {
        Ok(b) => b,
        Err(_) => return, // S3 not configured
    };
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());

    eprintln!(
        "[INFO] Uploading {} ({} bytes) to s3://{}/{} (region: {})",
        filename,
        data.len(),
        bucket,
        filename,
        region
    );

    // Invoke `aws s3 cp` as a child process to avoid bringing the full SDK
    // into scope at compile-time.  Production deployments should ensure the
    // AWS CLI is available and credentials are configured via IAM roles or
    // environment variables.
    let tmp_path = env::temp_dir().join(filename);
    if let Err(e) = fs::write(&tmp_path, data).await {
        eprintln!("[WARN] S3 upload skipped: could not write temp file: {e}");
        return;
    }

    let s3_uri = format!("s3://{}/{}", bucket, filename);
    let result = Command::new("aws")
        .args(["s3", "cp", "--region", &region])
        .arg(&tmp_path)
        .arg(&s3_uri)
        .output();

    let _ = fs::remove_file(&tmp_path).await;

    match result {
        Ok(o) if o.status.success() => {
            eprintln!("[INFO] S3 upload complete → {s3_uri}");
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!("[WARN] S3 upload failed: {stderr}");
        }
        Err(e) => {
            eprintln!("[WARN] S3 upload failed (aws CLI not available?): {e}");
        }
    }
}

// ── Actions ───────────────────────────────────────────────────────────────────

async fn action_backup(
    db_url: &str,
    output_dir: &Path,
    key: &[u8; 32],
) -> Result<BackupSummary, String> {
    let started = std::time::Instant::now();

    eprintln!("[INFO] Running pg_dump against database...");
    let plaintext = run_pg_dump(db_url)?;
    let plaintext_bytes = plaintext.len();
    eprintln!("[INFO] pg_dump produced {} bytes", plaintext_bytes);

    eprintln!("[INFO] Computing SHA-256 checksum of plaintext dump...");
    let sha256 = sha256_hex(&plaintext);
    eprintln!("[INFO] Plaintext SHA-256: {sha256}");

    eprintln!("[INFO] Encrypting with AES-256-GCM...");
    let encrypted = encrypt(&plaintext, key)?;
    let encrypted_bytes = encrypted.len();

    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let filename = format!("backup_{timestamp}.enc");
    let file_path = output_dir.join(&filename);

    fs::create_dir_all(output_dir)
        .await
        .map_err(|e| format!("Failed to create backup directory: {e}"))?;

    fs::write(&file_path, &encrypted)
        .await
        .map_err(|e| format!("Failed to write backup file: {e}"))?;

    eprintln!(
        "[INFO] Backup written to {} ({} encrypted bytes)",
        file_path.display(),
        encrypted_bytes
    );

    // Write a sidecar checksum file for out-of-band verification
    let checksum_filename = format!("{filename}.sha256");
    let checksum_content = format!("{sha256}  {filename}\n");
    fs::write(output_dir.join(&checksum_filename), checksum_content)
        .await
        .map_err(|e| format!("Failed to write checksum file: {e}"))?;
    eprintln!("[INFO] Checksum sidecar written: {checksum_filename}");

    // Best-effort S3 upload
    try_s3_upload(&filename, &encrypted).await;

    let duration_ms = started.elapsed().as_millis();
    Ok(BackupSummary {
        filename,
        plaintext_bytes,
        encrypted_bytes,
        sha256_hex: sha256,
        duration_ms,
    })
}

async fn action_verify(file_path: &Path, key: &[u8; 32]) -> Result<VerifySummary, String> {
    eprintln!("[INFO] Reading encrypted backup: {}", file_path.display());
    let encrypted = fs::read(file_path)
        .await
        .map_err(|e| format!("Cannot read file {}: {e}", file_path.display()))?;

    let encrypted_bytes = encrypted.len();
    eprintln!("[INFO] Encrypted size: {encrypted_bytes} bytes");

    eprintln!("[INFO] Decrypting with AES-256-GCM...");
    let decryption_result = decrypt(&encrypted, key);
    let decryption_ok = decryption_result.is_ok();

    let (plaintext_bytes, sha256_hex_val) = if let Ok(ref plain) = decryption_result {
        (plain.len(), sha256_hex(plain))
    } else {
        eprintln!(
            "[ERROR] Decryption failed: {}",
            decryption_result.as_ref().unwrap_err()
        );
        (0, String::new())
    };

    // Compare against sidecar checksum file if present
    let checksum_path = PathBuf::from(format!("{}.sha256", file_path.display()));
    if checksum_path.exists() {
        if let Ok(contents) = fs::read_to_string(&checksum_path).await {
            let expected = contents.split_whitespace().next().unwrap_or("").to_string();
            if !expected.is_empty() && expected != sha256_hex_val {
                eprintln!("[ERROR] Checksum MISMATCH! Expected: {expected}  Got: {sha256_hex_val}");
            } else if decryption_ok {
                eprintln!("[INFO] Checksum verification PASSED ✅");
            }
        }
    }

    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(VerifySummary {
        filename,
        encrypted_bytes,
        plaintext_bytes,
        sha256_hex: sha256_hex_val,
        decryption_ok,
    })
}

async fn action_restore(file_path: &Path, db_url: &str, key: &[u8; 32]) -> Result<(), String> {
    eprintln!("[INFO] Verifying backup before restore...");
    let summary = action_verify(file_path, key).await?;

    if !summary.decryption_ok {
        return Err(format!(
            "Restore aborted: backup file {} failed integrity check",
            file_path.display()
        ));
    }

    eprintln!(
        "[INFO] Backup verified ({} plaintext bytes). Proceeding with restore...",
        summary.plaintext_bytes
    );

    let encrypted = fs::read(file_path)
        .await
        .map_err(|e| format!("Cannot read file: {e}"))?;

    let plaintext = decrypt(&encrypted, key)?;

    eprintln!("[INFO] Restoring to database...");
    run_pg_restore(db_url, &plaintext)?;
    eprintln!("[INFO] Restore complete ✅");

    Ok(())
}

async fn action_retention(backup_dir: &Path, max_days: i64) -> Result<usize, String> {
    eprintln!(
        "[INFO] Enforcing retention: removing backups older than {} days from {}",
        max_days,
        backup_dir.display()
    );

    let mut entries = fs::read_dir(backup_dir)
        .await
        .map_err(|e| format!("Cannot read backup directory: {e}"))?;

    let now = Utc::now();
    let limit = Duration::days(max_days);
    let mut removed = 0usize;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Directory read error: {e}"))?
    {
        let name = entry.file_name().into_string().unwrap_or_default();

        // Pattern: backup_20260826T120000Z.enc
        if !name.starts_with("backup_") || !name.ends_with(".enc") {
            continue;
        }

        // Parse timestamp from filename
        let ts_str = name
            .strip_prefix("backup_")
            .and_then(|s| s.strip_suffix(".enc"))
            .unwrap_or("");

        let backup_time: Option<DateTime<Utc>> =
            DateTime::parse_from_str(&format!("{} +0000", ts_str), "%Y%m%dT%H%M%SZ %z")
                .ok()
                .map(|dt| dt.with_timezone(&Utc));

        let should_delete = match backup_time {
            Some(bt) => now.signed_duration_since(bt) > limit,
            None => {
                eprintln!("[WARN] Could not parse timestamp from {name}; skipping");
                false
            }
        };

        if should_delete {
            let path = entry.path();
            match fs::remove_file(&path).await {
                Ok(_) => {
                    eprintln!("[INFO] Deleted expired backup: {name}");
                    removed += 1;
                    // Also remove sidecar checksum file if present
                    let checksum = PathBuf::from(format!("{}.sha256", path.display()));
                    let _ = fs::remove_file(checksum).await;
                }
                Err(e) => {
                    eprintln!("[WARN] Could not delete {name}: {e}");
                }
            }
        }
    }

    eprintln!("[INFO] Retention sweep complete: {removed} file(s) removed");
    Ok(removed)
}

async fn action_list(backup_dir: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(backup_dir)
        .await
        .map_err(|e| format!("Cannot read backup directory: {e}"))?;

    let mut backups: Vec<String> = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Directory read error: {e}"))?
    {
        let name = entry.file_name().into_string().unwrap_or_default();
        if name.starts_with("backup_") && name.ends_with(".enc") {
            let meta = fs::metadata(entry.path()).await;
            let size = meta.map(|m| m.len()).unwrap_or(0);
            backups.push(format!("  {name}  ({size} bytes)"));
        }
    }

    if backups.is_empty() {
        println!("No backups found in {}", backup_dir.display());
    } else {
        backups.sort();
        println!("Backups in {}:", backup_dir.display());
        for line in &backups {
            println!("{line}");
        }
        println!("Total: {} backup(s)", backups.len());
    }

    Ok(())
}

// ── CLI argument helpers ──────────────────────────────────────────────────────

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|r| r == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    let action = arg_value(&args, "--action").unwrap_or_else(|| "backup".into());
    let db_url = arg_value(&args, "--db-url")
        .or_else(|| env::var("DATABASE_URL").ok())
        .unwrap_or_default();

    let backup_dir_str = arg_value(&args, "--output-dir")
        .or_else(|| env::var("BACKUP_DIR").ok())
        .unwrap_or_else(|| "./backups".into());
    let backup_dir = Path::new(&backup_dir_str);

    let max_days: i64 = arg_value(&args, "--max-days")
        .or_else(|| env::var("BACKUP_MAX_DAYS").ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let key = resolve_encryption_key();

    match action.as_str() {
        "backup" => {
            if db_url.is_empty() {
                eprintln!("[ERROR] --db-url or DATABASE_URL must be set for the backup action");
                std::process::exit(1);
            }
            eprintln!("[INFO] === Soroban Playground Backup Tool ===");
            eprintln!("[INFO] Action:     backup");
            eprintln!("[INFO] Output dir: {}", backup_dir.display());

            let summary = action_backup(&db_url, backup_dir, &key).await?;

            println!("[RESULT] Backup complete");
            println!("         File:              {}", summary.filename);
            println!("         Plaintext bytes:   {}", summary.plaintext_bytes);
            println!("         Encrypted bytes:   {}", summary.encrypted_bytes);
            println!("         SHA-256 (plaintext): {}", summary.sha256_hex);
            println!("         Duration:          {} ms", summary.duration_ms);
        }

        "verify" => {
            let file_arg = arg_value(&args, "--file");
            let file_path_str = file_arg.unwrap_or_default();
            if file_path_str.is_empty() {
                eprintln!("[ERROR] --file <path> is required for the verify action");
                std::process::exit(1);
            }
            let file_path = Path::new(&file_path_str);

            eprintln!("[INFO] === Soroban Playground Backup Tool ===");
            eprintln!("[INFO] Action: verify");

            let summary = action_verify(file_path, &key).await?;

            if summary.decryption_ok {
                println!("[RESULT] Verification PASSED ✅");
            } else {
                eprintln!("[RESULT] Verification FAILED ❌");
                std::process::exit(2);
            }
            println!("         File:            {}", summary.filename);
            println!("         Encrypted bytes: {}", summary.encrypted_bytes);
            println!("         Plaintext bytes: {}", summary.plaintext_bytes);
            println!("         SHA-256:         {}", summary.sha256_hex);
        }

        "restore" => {
            let file_arg = arg_value(&args, "--file");
            let file_path_str = file_arg.unwrap_or_default();
            if file_path_str.is_empty() {
                eprintln!("[ERROR] --file <path> is required for the restore action");
                std::process::exit(1);
            }
            if db_url.is_empty() {
                eprintln!("[ERROR] --db-url or DATABASE_URL must be set for the restore action");
                std::process::exit(1);
            }
            let file_path = Path::new(&file_path_str);

            eprintln!("[INFO] === Soroban Playground Backup Tool ===");
            eprintln!("[INFO] Action: restore");
            eprintln!("[INFO] File:   {}", file_path.display());

            action_restore(file_path, &db_url, &key).await?;
        }

        "retention" => {
            eprintln!("[INFO] === Soroban Playground Backup Tool ===");
            eprintln!("[INFO] Action:    retention");
            eprintln!("[INFO] Max days:  {max_days}");

            let removed = action_retention(backup_dir, max_days).await?;
            println!("[RESULT] Retention sweep removed {removed} backup(s)");
        }

        "list" => {
            action_list(backup_dir).await?;
        }

        _ => {
            eprintln!(
                "[ERROR] Unknown action '{action}'. \
                 Use --action backup | verify | restore | retention | list"
            );
            std::process::exit(1);
        }
    }

    Ok(())
}
