/// Storage abstraction — routes file I/O through either local filesystem
/// or DarwinKit's coordinated iCloud methods depending on the active mode.
///
/// When iCloud is enabled, all file operations go through NSFileCoordinator
/// via DarwinKit JSON-RPC. When local or custom, direct std::fs (current behavior).
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use super::darwinkit;
use super::settings;

// ── Storage Mode ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageMode {
    Local,
    ICloud,
    Custom(String),
}

/// Determine the active storage mode from settings.
/// Priority: icloud.enabled > notes_directory (custom) > local default.
pub fn current_mode() -> StorageMode {
    match settings::load_settings_from_file() {
        Ok(s) => {
            if s.icloud.enabled {
                StorageMode::ICloud
            } else if !s.notes_directory.is_empty() {
                let p = PathBuf::from(&s.notes_directory);
                if p.is_absolute() {
                    StorageMode::Custom(s.notes_directory)
                } else {
                    StorageMode::Local
                }
            } else {
                StorageMode::Local
            }
        }
        Err(_) => StorageMode::Local,
    }
}

/// Get the root Stik directory for the current storage mode.
pub fn stik_root() -> Result<PathBuf, String> {
    match current_mode() {
        StorageMode::ICloud => icloud_stik_root(),
        StorageMode::Custom(dir) => {
            let path = PathBuf::from(&dir).join("Stik");
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok(path)
        }
        StorageMode::Local => {
            let docs = dirs::document_dir().ok_or("Could not find Documents directory")?;
            let path = docs.join("Stik");
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok(path)
        }
    }
}

/// Well-known macOS iCloud container path for Stik.
/// On macOS, iCloud Drive containers are stored at a deterministic path
/// under ~/Library/Mobile Documents/. We resolve this from Rust directly
/// instead of going through the DarwinKit sidecar, because the sidecar's
/// FileManager.url(forUbiquityContainerIdentifier:) requires proper code
/// signing with iCloud entitlements (not ad-hoc signing).
const ICLOUD_CONTAINER_FOLDER: &str = "iCloud~com~0xmassi~stik";

/// Get the iCloud container base path (parent of Documents/Stik).
pub fn icloud_container_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let mobile_docs = home.join("Library").join("Mobile Documents");
    let container = mobile_docs.join(ICLOUD_CONTAINER_FOLDER);
    Ok(container)
}

/// Check whether the iCloud container exists on disk, meaning iCloud Drive
/// is enabled and the container has been provisioned.
pub fn icloud_available() -> bool {
    icloud_container_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Resolve the iCloud container's Stik root using the well-known macOS path.
fn icloud_stik_root() -> Result<PathBuf, String> {
    let container = icloud_container_path()?;

    if !container.exists() {
        return Err(
            "iCloud container not available. Please enable iCloud Drive in System Settings.".to_string()
        );
    }

    let path = container.join("Documents").join("Stik");
    fs::create_dir_all(&path).map_err(|e| format!("Failed to create iCloud Stik directory: {}", e))?;
    Ok(path)
}

// ── File Operations ───────────────────────────────────────────────

pub fn read_file(path: &str) -> Result<String, String> {
    match current_mode() {
        StorageMode::ICloud => {
            let result = darwinkit::call_with_timeout(
                "icloud.read",
                Some(serde_json::json!({ "path": path })),
                30,
            )?;
            result
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| "iCloud read returned no content".to_string())
        }
        _ => fs::read_to_string(path).map_err(|e| e.to_string()),
    }
}

pub fn write_file(path: &str, content: &str) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.write",
                Some(serde_json::json!({ "path": path, "content": content })),
                30,
            )?;
            Ok(())
        }
        _ => fs::write(path, content).map_err(|e| e.to_string()),
    }
}

pub fn write_bytes(path: &str, data: &[u8]) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            darwinkit::call_with_timeout(
                "icloud.write_bytes",
                Some(serde_json::json!({ "path": path, "data": b64 })),
                30,
            )?;
            Ok(())
        }
        _ => fs::write(path, data).map_err(|e| e.to_string()),
    }
}

pub fn delete_file(path: &str) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.delete",
                Some(serde_json::json!({ "path": path })),
                30,
            )?;
            Ok(())
        }
        _ => fs::remove_file(path).map_err(|e| e.to_string()),
    }
}

pub fn move_file(src: &str, dst: &str) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.move",
                Some(serde_json::json!({ "source": src, "destination": dst })),
                30,
            )?;
            Ok(())
        }
        _ => fs::rename(src, dst).map_err(|e| e.to_string()),
    }
}

pub fn copy_file(src: &str, dst: &str) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.copy_file",
                Some(serde_json::json!({ "source": src, "destination": dst })),
                30,
            )?;
            Ok(())
        }
        _ => {
            fs::copy(src, dst).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

pub fn ensure_dir(path: &str) -> Result<(), String> {
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.ensure_dir",
                Some(serde_json::json!({ "path": path })),
                30,
            )?;
            Ok(())
        }
        _ => fs::create_dir_all(path).map_err(|e| e.to_string()),
    }
}

pub fn remove_dir_all(path: &str) -> Result<(), String> {
    // No special iCloud handling needed — coordinated delete works on directories too
    match current_mode() {
        StorageMode::ICloud => {
            darwinkit::call_with_timeout(
                "icloud.delete",
                Some(serde_json::json!({ "path": path })),
                30,
            )?;
            Ok(())
        }
        _ => fs::remove_dir_all(path).map_err(|e| e.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified: Option<String>,
}

pub fn list_dir(path: &str) -> Result<Vec<DirEntry>, String> {
    match current_mode() {
        StorageMode::ICloud => {
            let result = darwinkit::call_with_timeout(
                "icloud.list_dir",
                Some(serde_json::json!({ "path": path })),
                30,
            )?;
            let entries = result
                .get("entries")
                .and_then(|v| v.as_array())
                .ok_or("iCloud list_dir returned no entries")?;

            Ok(entries
                .iter()
                .filter_map(|e| {
                    Some(DirEntry {
                        name: e.get("name")?.as_str()?.to_string(),
                        is_directory: e.get("is_directory")?.as_bool()?,
                        size: e.get("size")?.as_u64().unwrap_or(0),
                        modified: e.get("modified").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    })
                })
                .collect())
        }
        _ => {
            let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
            Ok(entries
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let metadata = entry.metadata().ok()?;
                    let modified = metadata
                        .modified()
                        .ok()
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        });
                    Some(DirEntry {
                        name: entry.file_name().to_string_lossy().to_string(),
                        is_directory: metadata.is_dir(),
                        size: metadata.len(),
                        modified,
                    })
                })
                .collect())
        }
    }
}

/// Check if a path exists. For iCloud mode, attempts a list_dir on the parent
/// to verify the file. For local, uses std::path::Path::exists().
pub fn path_exists(path: &str) -> bool {
    match current_mode() {
        StorageMode::ICloud => {
            // For iCloud, try reading — if it fails, the file doesn't exist
            // This is simpler than listing the parent directory
            let p = PathBuf::from(path);
            if p.is_dir() {
                list_dir(path).is_ok()
            } else {
                read_file(path).is_ok()
            }
        }
        _ => PathBuf::from(path).exists(),
    }
}

/// Check if path is a directory. For local mode only (iCloud uses list_dir).
pub fn is_dir(path: &str) -> bool {
    match current_mode() {
        StorageMode::ICloud => {
            // Try listing — if it succeeds, it's a directory
            list_dir(path).is_ok()
        }
        _ => PathBuf::from(path).is_dir(),
    }
}

/// Start iCloud file monitoring via DarwinKit
pub fn start_monitoring() -> Result<(), String> {
    if current_mode() != StorageMode::ICloud {
        return Ok(());
    }
    darwinkit::call("icloud.start_monitoring", None)?;
    Ok(())
}

/// Stop iCloud file monitoring
pub fn stop_monitoring() -> Result<(), String> {
    darwinkit::call("icloud.stop_monitoring", None)?;
    Ok(())
}
