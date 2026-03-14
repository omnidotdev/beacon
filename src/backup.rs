//! Backup and restore for Beacon data

use std::path::{Path, PathBuf};

use chrono::Utc;

/// Result of backup verification
#[derive(Debug)]
pub struct BackupVerification {
    /// Whether the backup is valid
    pub valid: bool,
    /// List of files found in the backup
    pub files: Vec<String>,
    /// Any issues found
    pub issues: Vec<String>,
}

/// Create a backup tarball of the data directory
///
/// Includes: `SQLite` database, config.toml
///
/// # Errors
///
/// Returns error if backup creation fails
pub fn create_backup(data_dir: &Path, output: Option<&Path>) -> anyhow::Result<PathBuf> {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let default_name = format!("beacon-backup-{timestamp}.tar.gz");
    let output_path = output.map_or_else(
        || {
            std::env::current_dir()
                .unwrap_or_default()
                .join(&default_name)
        },
        Path::to_path_buf,
    );

    let file = std::fs::File::create(&output_path)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    // Back up database using SQLite backup API for consistency
    let db_path = data_dir.join("beacon.db");
    if db_path.exists() {
        let backup_tmp = tempfile::NamedTempFile::new()?;
        {
            let src = rusqlite::Connection::open(&db_path)?;
            let mut dst = rusqlite::Connection::open(backup_tmp.path())?;
            let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
            backup.run_to_completion(100, std::time::Duration::from_millis(50), None)?;
        }
        tar.append_path_with_name(backup_tmp.path(), "beacon.db")?;
    }

    // Back up config file
    if let Some(config_path) = crate::config::file::config_file_path()
        && config_path.exists()
    {
        tar.append_path_with_name(&config_path, "config.toml")?;
    }

    // Back up skills directory
    let skills_dir = data_dir.join("skills");
    if skills_dir.is_dir() {
        tar.append_dir_all("skills", &skills_dir)?;
    }

    tar.finish()?;

    println!("backup created: {}", output_path.display());

    Ok(output_path)
}

/// Verify a backup tarball
///
/// # Errors
///
/// Returns error if the backup cannot be read
pub fn verify_backup(path: &Path) -> anyhow::Result<BackupVerification> {
    let file = std::fs::File::open(path)?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(dec);

    let mut files = Vec::new();
    let mut issues = Vec::new();
    let mut has_db = false;

    for entry in archive.entries()? {
        let entry = entry?;
        let entry_path = entry.path()?;
        let name = entry_path.display().to_string();

        if name == "beacon.db" {
            has_db = true;
        }

        files.push(name);
    }

    if !has_db {
        issues.push("backup does not contain beacon.db".to_string());
    }

    let valid = issues.is_empty();

    println!(
        "backup {}: {} files, {}",
        if valid { "valid" } else { "INVALID" },
        files.len(),
        if issues.is_empty() {
            "no issues".to_string()
        } else {
            format!("{} issue(s)", issues.len())
        }
    );

    for file in &files {
        println!("  {file}");
    }

    for issue in &issues {
        println!("  [!] {issue}");
    }

    Ok(BackupVerification {
        valid,
        files,
        issues,
    })
}

/// Restore from a backup tarball
///
/// # Errors
///
/// Returns error if restoration fails
pub fn restore_backup(path: &Path, data_dir: &Path) -> anyhow::Result<()> {
    // Verify first
    let verification = verify_backup(path)?;
    if !verification.valid {
        anyhow::bail!(
            "backup verification failed: {}",
            verification.issues.join(", ")
        );
    }

    // Ensure data dir exists
    std::fs::create_dir_all(data_dir)?;

    // Extract
    let file = std::fs::File::open(path)?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(dec);

    archive.unpack(data_dir)?;

    println!("backup restored to {}", data_dir.display());

    Ok(())
}
