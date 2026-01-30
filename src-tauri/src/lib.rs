use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};
#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use sysinfo::Disks;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{
    SHEmptyRecycleBinW, SHQueryRecycleBinW, SHQUERYRBINFO, SHERB_NOCONFIRMATION,
    SHERB_NOPROGRESSUI, SHERB_NOSOUND,
};
use walkdir::WalkDir;
use tauri::Manager;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiskInfo {
    mount_point: String,
    total_bytes: u64,
    free_bytes: u64,
    used_bytes: u64,
    used_percent: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupCategory {
    id: String,
    title: String,
    description: String,
    size_bytes: u64,
    file_count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupItem {
    path: String,
    size_bytes: u64,
    modified_ms: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HibernationInfo {
    enabled: bool,
    size_bytes: u64,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LargeItem {
    path: String,
    name: String,
    size_bytes: u64,
    is_dir: bool,
    suspicious: bool,
    category_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryItems {
    items: Vec<CleanupItem>,
    has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupError {
    path: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupResult {
    deleted_bytes: u64,
    deleted_count: u64,
    failed: Vec<CleanupError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryStats {
    size_bytes: u64,
    file_count: u64,
}

#[cfg(target_os = "windows")]
struct RecycleBinStats {
    deleted_bytes: u64,
    deleted_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanRequest {
    ids: Vec<String>,
    #[serde(default)]
    excluded_paths: HashMap<String, Vec<String>>,
    #[serde(default)]
    included_paths: HashMap<String, Vec<String>>,
    #[serde(default)]
    category_stats: HashMap<String, CategoryStats>,
}

#[derive(Clone)]
enum CategoryKind {
    Standard,
    DownloadsOlderThan { days: u64 },
}

#[derive(Clone)]
struct CategoryDef {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    kind: CategoryKind,
    roots: Vec<PathBuf>,
    cleanup_dirs: bool,
}

#[tauri::command]
async fn get_disk_info() -> Result<DiskInfo, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || get_disk_info_sync())
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn get_hibernation_info() -> Result<HibernationInfo, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || get_hibernation_info_sync())
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn set_hibernation_enabled(enabled: bool) -> Result<HibernationInfo, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || set_hibernation_enabled_sync(enabled))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn scan_cleanup_items() -> Result<Vec<CleanupCategory>, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || scan_cleanup_items_sync())
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn scan_large_items(limit: Option<u32>, min_size_mb: Option<u64>) -> Result<Vec<LargeItem>, String> {
    ensure_windows()?;
    let limit = limit.unwrap_or(200).min(1000) as usize;
    let min_size_bytes = min_size_mb
        .unwrap_or(1024)
        .saturating_mul(1024)
        .saturating_mul(1024);
    tauri::async_runtime::spawn_blocking(move || scan_large_items_sync(limit, min_size_bytes))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn list_category_items(id: String, limit: Option<u32>) -> Result<CategoryItems, String> {
    ensure_windows()?;
    let limit = limit.unwrap_or(200).min(2000) as usize;
    tauri::async_runtime::spawn_blocking(move || list_category_items_sync(id, limit))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn clean_categories(request: CleanRequest) -> Result<CleanupResult, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || clean_categories_sync(request))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn clean_large_items(paths: Vec<String>) -> Result<CleanupResult, String> {
    ensure_windows()?;
    let result = tauri::async_runtime::spawn_blocking(move || clean_large_items_sync(paths))
        .await
        .map_err(|err| err.to_string())?;
    Ok(result)
}

fn ensure_windows() -> Result<(), String> {
    if cfg!(target_os = "windows") {
        Ok(())
    } else {
        Err("This app currently supports Windows only.".to_string())
    }
}

fn get_disk_info_sync() -> Result<DiskInfo, String> {
    let mount_point = system_drive_mount();
    let disks = Disks::new_with_refreshed_list();
    let disk_list = disks.list();
    let disk = disk_list
        .iter()
        .find(|disk| path_eq_ignore_case(disk.mount_point(), &mount_point))
        .or_else(|| disk_list.first())
        .ok_or_else(|| "No disks detected.".to_string())?;

    let total = disk.total_space();
    let free = disk.available_space();
    let used = total.saturating_sub(free);
    let used_percent = if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    };

    Ok(DiskInfo {
        mount_point: disk.mount_point().to_string_lossy().to_string(),
        total_bytes: total,
        free_bytes: free,
        used_bytes: used,
        used_percent,
    })
}

fn get_hibernation_info_sync() -> Result<HibernationInfo, String> {
    let system_drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let path = PathBuf::from(format!("{}\\hiberfil.sys", system_drive));
    let (enabled, size_bytes) = match fs::metadata(&path) {
        Ok(metadata) => (true, metadata.len()),
        Err(err) if err.kind() == ErrorKind::NotFound => (false, 0),
        Err(err) => return Err(err.to_string()),
    };
    Ok(HibernationInfo {
        enabled,
        size_bytes,
        path: path.to_string_lossy().to_string(),
    })
}

fn set_hibernation_enabled_sync(enabled: bool) -> Result<HibernationInfo, String> {
    let status = Command::new("powercfg")
        .arg("/hibernate")
        .arg(if enabled { "on" } else { "off" })
        .status()
        .map_err(|err| err.to_string())?;
    if !status.success() {
        return Err("Failed to update hibernation state. Try running as administrator.".to_string());
    }
    get_hibernation_info_sync()
}

fn scan_cleanup_items_sync() -> Result<Vec<CleanupCategory>, String> {
    let categories = build_categories();
    let items = categories
        .iter()
        .map(|def| {
            let scan = scan_category(def);
            CleanupCategory {
                id: def.id.to_string(),
                title: def.title.to_string(),
                description: def.description.to_string(),
                size_bytes: scan.size_bytes,
                file_count: scan.file_count,
            }
        })
        .collect();
    Ok(items)
}

fn scan_large_items_sync(limit: usize, min_size_bytes: u64) -> Result<Vec<LargeItem>, String> {
    let root = system_drive_mount();
    let categories = build_categories();
    let keywords = ["log", "cache", "temp", "tmp"];
    let mut large_files = Vec::new();
    let mut suspicious_dirs: HashMap<String, (PathBuf, u64)> = HashMap::new();

    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };
        let size = metadata.len();
        let path = entry.path();

        if size >= min_size_bytes {
            let name = entry.file_name().to_string_lossy().to_string();
            let path_text = path.to_string_lossy();
            let suspicious =
                contains_keyword(&name, &keywords) || contains_keyword(&path_text, &keywords);
            let category_id = match_category_id(path, &metadata, &categories);
            large_files.push(LargeItem {
                path: path.to_string_lossy().to_string(),
                name,
                size_bytes: size,
                is_dir: false,
                suspicious,
                category_id,
            });
        }

        if let Some(suspicious_dir) = find_suspicious_dir(path.parent(), &keywords) {
            let key = normalize_path(&suspicious_dir);
            let entry = suspicious_dirs
                .entry(key)
                .or_insert((suspicious_dir, 0));
            entry.1 = entry.1.saturating_add(size);
        }
    }

    let mut large_dirs = Vec::new();
    for (_, (path, size)) in suspicious_dirs {
        if size < min_size_bytes {
            continue;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        large_dirs.push(LargeItem {
            path: path.to_string_lossy().to_string(),
            name,
            size_bytes: size,
            is_dir: true,
            suspicious: true,
            category_id: None,
        });
    }

    let mut items = Vec::with_capacity(large_files.len() + large_dirs.len());
    items.extend(large_files);
    items.extend(large_dirs);
    items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    if items.len() > limit {
        items.truncate(limit);
    }
    Ok(items)
}

fn list_category_items_sync(id: String, limit: usize) -> Result<CategoryItems, String> {
    let categories = build_categories();
    let def = categories
        .iter()
        .find(|category| category.id == id)
        .ok_or_else(|| "Unknown cleanup category.".to_string())?;
    Ok(list_category_items_for(def, limit))
}

fn clean_categories_sync(request: CleanRequest) -> Result<CleanupResult, String> {
    let categories = build_categories();
    let CleanRequest {
        ids,
        excluded_paths,
        included_paths,
        category_stats,
    } = request;
    let id_set: HashSet<String> = ids.into_iter().collect();
    let mut deleted_bytes = 0;
    let mut deleted_count = 0;
    let mut failed = Vec::new();

    for def in categories.iter() {
        let included = included_paths
            .get(def.id)
            .cloned()
            .unwrap_or_default();
        if !included.is_empty() {
            let result = clean_included_paths(def, &included);
            deleted_bytes += result.deleted_bytes;
            deleted_count += result.deleted_count;
            failed.extend(result.failed);
            continue;
        }
        if !id_set.contains(def.id) {
            continue;
        }
        let excluded = excluded_paths
            .get(def.id)
            .map(normalize_exclusions)
            .unwrap_or_default();
        let stats = category_stats.get(def.id);
        let result = clean_category(def, &excluded, stats);
        deleted_bytes += result.deleted_bytes;
        deleted_count += result.deleted_count;
        failed.extend(result.failed);
    }

    Ok(CleanupResult {
        deleted_bytes,
        deleted_count,
        failed,
    })
}

fn clean_large_items_sync(paths: Vec<String>) -> CleanupResult {
    let root = system_drive_mount();
    let mut deleted_bytes: u64 = 0;
    let mut deleted_count: u64 = 0;
    let mut failed = Vec::new();
    let mut seen = HashSet::new();

    for path_str in paths {
        let normalized = normalize_path_str(&path_str);
        if !seen.insert(normalized) {
            continue;
        }
        let path = Path::new(&path_str);
        if !is_within_root(&root, path) {
            failed.push(CleanupError {
                path: path_str.clone(),
                message: "Path is outside scan scope.".to_string(),
            });
            continue;
        }
        if path_eq_ignore_case(path, &root) {
            failed.push(CleanupError {
                path: path_str.clone(),
                message: "Refusing to delete drive root.".to_string(),
            });
            continue;
        }
        let metadata = match path.metadata() {
            Ok(meta) => meta,
            Err(err) => {
                failed.push(CleanupError {
                    path: path_str.clone(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        if metadata.is_dir() {
            let (size, count) = dir_metrics(path);
            if let Err(err) = fs::remove_dir_all(path) {
                failed.push(CleanupError {
                    path: path_str.clone(),
                    message: err.to_string(),
                });
                continue;
            }
            deleted_bytes = deleted_bytes.saturating_add(size);
            deleted_count = deleted_count.saturating_add(count);
        } else {
            let size = metadata.len();
            if let Err(err) = fs::remove_file(path) {
                failed.push(CleanupError {
                    path: path_str.clone(),
                    message: err.to_string(),
                });
                continue;
            }
            deleted_bytes = deleted_bytes.saturating_add(size);
            deleted_count = deleted_count.saturating_add(1);
        }
    }

    CleanupResult {
        deleted_bytes,
        deleted_count,
        failed,
    }
}

struct CategoryScan {
    size_bytes: u64,
    file_count: u64,
}

fn scan_category(def: &CategoryDef) -> CategoryScan {
    let cutoff = cutoff_time(&def.kind);
    let mut size_bytes = 0;
    let mut file_count = 0;

    for root in &def.roots {
        size_bytes += scan_root(root, cutoff, &mut file_count);
    }

    CategoryScan {
        size_bytes,
        file_count,
    }
}

fn scan_root(root: &Path, cutoff: Option<SystemTime>, file_count: &mut u64) -> u64 {
    if !root.exists() {
        return 0;
    }

    let mut size_bytes = 0;
    if root.is_file() {
        if let Ok(metadata) = root.metadata() {
            if matches_cutoff(&metadata, cutoff) {
                size_bytes += metadata.len();
                *file_count += 1;
            }
        }
        return size_bytes;
    }

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            if matches_cutoff(&metadata, cutoff) {
                size_bytes += metadata.len();
                *file_count += 1;
            }
        }
    }

    size_bytes
}

fn list_category_items_for(def: &CategoryDef, limit: usize) -> CategoryItems {
    let cutoff = cutoff_time(&def.kind);
    let mut items = Vec::new();
    let mut has_more = false;

    for root in &def.roots {
        if items.len() >= limit {
            has_more = true;
            break;
        }
        if !root.exists() {
            continue;
        }
        if root.is_file() {
            if let Ok(metadata) = root.metadata() {
                if matches_cutoff(&metadata, cutoff) {
                    items.push(to_item(root, &metadata));
                }
            }
            continue;
        }

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if items.len() >= limit {
                has_more = true;
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                if matches_cutoff(&metadata, cutoff) {
                    items.push(to_item(entry.path(), &metadata));
                }
            }
        }
    }

    CategoryItems { items, has_more }
}

fn clean_category(
    def: &CategoryDef,
    excluded: &HashSet<String>,
    stats: Option<&CategoryStats>,
) -> CleanupResult {
    if def.id == "recycle_bin" && excluded.is_empty() {
        return clean_recycle_bin_fast();
    }
    if excluded.is_empty() && should_fast_clear(def) {
        return clean_category_fast_dirs(def, stats);
    }
    let cutoff = cutoff_time(&def.kind);
    let mut deleted_bytes = 0;
    let mut deleted_count = 0;
    let mut failed = Vec::new();
    let mut dirs = Vec::new();

    for root in &def.roots {
        if !root.exists() {
            continue;
        }
        if root.is_file() {
            delete_file(root, cutoff, excluded, &mut deleted_bytes, &mut deleted_count, &mut failed);
            continue;
        }

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_dir() {
                if def.cleanup_dirs {
                    dirs.push(entry.path().to_path_buf());
                }
                continue;
            }
            if entry.file_type().is_file() {
                delete_file(
                    entry.path(),
                    cutoff,
                    excluded,
                    &mut deleted_bytes,
                    &mut deleted_count,
                    &mut failed,
                );
            }
        }

        if def.cleanup_dirs {
            dirs.push(root.to_path_buf());
        }
    }

    if def.cleanup_dirs && !dirs.is_empty() {
        dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for dir in dirs {
            let _ = fs::remove_dir(&dir);
        }
    }

    CleanupResult {
        deleted_bytes,
        deleted_count,
        failed,
    }
}

fn clean_included_paths(def: &CategoryDef, included: &[String]) -> CleanupResult {
    let cutoff = cutoff_time(&def.kind);
    let mut deleted_bytes = 0;
    let mut deleted_count = 0;
    let mut failed = Vec::new();
    let mut seen = HashSet::new();

    for path_str in included {
        let normalized = normalize_path_str(path_str);
        if !seen.insert(normalized) {
            continue;
        }
        let path = Path::new(path_str);
        if !is_within_roots(def, path) {
            failed.push(CleanupError {
                path: path_str.clone(),
                message: "Path is outside cleanup scope.".to_string(),
            });
            continue;
        }
        let metadata = match path.metadata() {
            Ok(meta) => meta,
            Err(err) => {
                failed.push(CleanupError {
                    path: path_str.clone(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        if metadata.is_dir() {
            failed.push(CleanupError {
                path: path_str.clone(),
                message: "Path is a directory.".to_string(),
            });
            continue;
        }
        if !matches_cutoff(&metadata, cutoff) {
            continue;
        }
        let size = metadata.len();
        if let Err(err) = fs::remove_file(path) {
            failed.push(CleanupError {
                path: path_str.clone(),
                message: err.to_string(),
            });
            continue;
        }
        deleted_bytes += size;
        deleted_count += 1;
    }

    CleanupResult {
        deleted_bytes,
        deleted_count,
        failed,
    }
}

fn should_fast_clear(def: &CategoryDef) -> bool {
    matches!(def.id, "system_cache" | "browser_cache") && def.cleanup_dirs
}

fn clean_category_fast_dirs(def: &CategoryDef, stats: Option<&CategoryStats>) -> CleanupResult {
    let mut failed = Vec::new();
    let mut all_ok = true;

    for root in &def.roots {
        if !root.exists() {
            continue;
        }
        let result = if root.is_file() {
            fs::remove_file(root).map_err(|err| err.to_string())
        } else {
            fs::remove_dir_all(root).map_err(|err| err.to_string())
        };
        if let Err(message) = result {
            all_ok = false;
            failed.push(CleanupError {
                path: root.to_string_lossy().to_string(),
                message,
            });
        }
    }

    let (deleted_bytes, deleted_count) = if all_ok {
        stats
            .map(|value| (value.size_bytes, value.file_count))
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    CleanupResult {
        deleted_bytes,
        deleted_count,
        failed,
    }
}

fn clean_recycle_bin_fast() -> CleanupResult {
    #[cfg(target_os = "windows")]
    {
        let stats = query_recycle_bin_stats(None).unwrap_or(RecycleBinStats {
            deleted_bytes: 0,
            deleted_count: 0,
        });
        let mut failed = Vec::new();
        if let Err(err) = empty_recycle_bin(None) {
            failed.push(CleanupError {
                path: "$Recycle.Bin".to_string(),
                message: err,
            });
            return CleanupResult {
                deleted_bytes: 0,
                deleted_count: 0,
                failed,
            };
        }
        CleanupResult {
            deleted_bytes: stats.deleted_bytes,
            deleted_count: stats.deleted_count,
            failed,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        CleanupResult {
            deleted_bytes: 0,
            deleted_count: 0,
            failed: vec![CleanupError {
                path: "$Recycle.Bin".to_string(),
                message: "Recycle bin fast clear is only supported on Windows.".to_string(),
            }],
        }
    }
}

#[cfg(target_os = "windows")]
fn query_recycle_bin_stats(root: Option<&Path>) -> Result<RecycleBinStats, String> {
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        i64Size: 0,
        i64NumItems: 0,
    };
    let wide = root.map(to_wide_null);
    let root_ptr = wide
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    let hr = unsafe { SHQueryRecycleBinW(root_ptr, &mut info) };
    if hr < 0 {
        return Err(format!("SHQueryRecycleBinW failed: 0x{:08X}", hr as u32));
    }
    Ok(RecycleBinStats {
        deleted_bytes: info.i64Size as u64,
        deleted_count: info.i64NumItems as u64,
    })
}

#[cfg(target_os = "windows")]
fn empty_recycle_bin(root: Option<&Path>) -> Result<(), String> {
    let wide = root.map(to_wide_null);
    let root_ptr = wide
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
    let hr = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), root_ptr, flags) };
    if hr < 0 {
        return Err(format!("SHEmptyRecycleBinW failed: 0x{:08X}", hr as u32));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn to_wide_null(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn is_within_roots(def: &CategoryDef, path: &Path) -> bool {
    let target = normalize_path(path);
    def.roots.iter().any(|root| {
        let root_norm = normalize_path(root);
        if root.is_file() {
            return target == root_norm;
        }
        let prefix = if root_norm.ends_with('\\') {
            root_norm.clone()
        } else {
            format!("{}\\", root_norm)
        };
        target == root_norm || target.starts_with(&prefix)
    })
}

fn delete_file(
    path: &Path,
    cutoff: Option<SystemTime>,
    excluded: &HashSet<String>,
    deleted_bytes: &mut u64,
    deleted_count: &mut u64,
    failed: &mut Vec<CleanupError>,
) {
    let normalized = normalize_path(path);
    if excluded.contains(&normalized) {
        return;
    }
    let metadata = match path.metadata() {
        Ok(meta) => meta,
        Err(err) => {
            failed.push(CleanupError {
                path: path.to_string_lossy().to_string(),
                message: err.to_string(),
            });
            return;
        }
    };
    if !matches_cutoff(&metadata, cutoff) {
        return;
    }
    let size = metadata.len();
    if let Err(err) = fs::remove_file(path) {
        failed.push(CleanupError {
            path: path.to_string_lossy().to_string(),
            message: err.to_string(),
        });
        return;
    }
    *deleted_bytes += size;
    *deleted_count += 1;
}

fn to_item(path: &Path, metadata: &fs::Metadata) -> CleanupItem {
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    CleanupItem {
        path: path.to_string_lossy().to_string(),
        size_bytes: metadata.len(),
        modified_ms,
    }
}

fn matches_cutoff(metadata: &fs::Metadata, cutoff: Option<SystemTime>) -> bool {
    match cutoff {
        Some(cutoff) => metadata
            .modified()
            .ok()
            .map(|modified| modified < cutoff)
            .unwrap_or(false),
        None => true,
    }
}

fn cutoff_time(kind: &CategoryKind) -> Option<SystemTime> {
    match kind {
        CategoryKind::Standard => None,
        CategoryKind::DownloadsOlderThan { days } => SystemTime::now()
            .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
            .or(Some(SystemTime::UNIX_EPOCH)),
    }
}

fn normalize_exclusions(exclusions: &Vec<String>) -> HashSet<String> {
    exclusions
        .iter()
        .map(|path| normalize_path_str(path))
        .collect()
}

fn normalize_path(path: &Path) -> String {
    normalize_path_str(&path.to_string_lossy())
}

fn normalize_path_str(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

fn is_within_root(root: &Path, path: &Path) -> bool {
    let root_norm = normalize_path(root);
    let target = normalize_path(path);
    if root.is_file() {
        return target == root_norm;
    }
    let prefix = if root_norm.ends_with('\\') {
        root_norm
    } else {
        format!("{}\\", root_norm)
    };
    target == prefix.trim_end_matches('\\') || target.starts_with(&prefix)
}

fn contains_keyword(text: &str, keywords: &[&str]) -> bool {
    let lowered = text.to_lowercase();
    keywords.iter().any(|keyword| lowered.contains(keyword))
}

fn find_suspicious_dir(path: Option<&Path>, keywords: &[&str]) -> Option<PathBuf> {
    let mut current = path?;
    loop {
        if let Some(name) = current.file_name().and_then(|value| value.to_str()) {
            if contains_keyword(name, keywords) {
                return Some(current.to_path_buf());
            }
        }
        current = current.parent()?;
    }
}

fn match_category_id(
    path: &Path,
    metadata: &fs::Metadata,
    categories: &[CategoryDef],
) -> Option<String> {
    for def in categories {
        if !is_within_roots(def, path) {
            continue;
        }
        let cutoff = cutoff_time(&def.kind);
        if !matches_cutoff(metadata, cutoff) {
            continue;
        }
        return Some(def.id.to_string());
    }
    None
}

fn dir_metrics(path: &Path) -> (u64, u64) {
    let mut size: u64 = 0;
    let mut count: u64 = 0;
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            size = size.saturating_add(metadata.len());
            count = count.saturating_add(1);
        }
    }
    (size, count)
}

fn path_eq_ignore_case(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn build_categories() -> Vec<CategoryDef> {
    let system_drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let system_root = env::var("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(format!("{}\\Windows", system_drive)));
    let user_profile = env::var("USERPROFILE").ok().map(PathBuf::from);
    let local_app_data = env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let temp_dir = env::temp_dir();

    let mut temp_paths = vec![system_root.join("Temp"), temp_dir];
    if let Some(local) = &local_app_data {
        temp_paths.push(local.join("Temp"));
    }

    let mut browser_paths = Vec::new();
    if let Some(local) = &local_app_data {
        browser_paths.extend([
            local.join("Microsoft").join("Windows").join("INetCache"),
            local
                .join("Google")
                .join("Chrome")
                .join("User Data")
                .join("Default")
                .join("Cache"),
            local
                .join("Google")
                .join("Chrome")
                .join("User Data")
                .join("Default")
                .join("Code Cache"),
            local
                .join("Microsoft")
                .join("Edge")
                .join("User Data")
                .join("Default")
                .join("Cache"),
            local
                .join("Microsoft")
                .join("Edge")
                .join("User Data")
                .join("Default")
                .join("Code Cache"),
        ]);
    }

    let recycle_bins = {
        let disks = Disks::new_with_refreshed_list();
        let mut roots = disks
            .list()
            .iter()
            .map(|disk| disk.mount_point().join("$Recycle.Bin"))
            .collect::<Vec<_>>();
        if roots.is_empty() {
            roots.push(PathBuf::from(format!("{}\\$Recycle.Bin", system_drive)));
        }
        roots
    };
    let windows_old = PathBuf::from(format!("{}\\Windows.old", system_drive));

    let download_root = user_profile
        .as_ref()
        .map(|profile| profile.join("Downloads"))
        .into_iter()
        .collect::<Vec<_>>();

    vec![
        CategoryDef {
            id: "temp_files",
            title: "临时文件",
            description: "Windows 和应用程序创建的临时文件",
            kind: CategoryKind::Standard,
            roots: dedup_paths(temp_paths),
            cleanup_dirs: true,
        },
        CategoryDef {
            id: "recycle_bin",
            title: "回收站",
            description: "清空回收站中的所有文件",
            kind: CategoryKind::Standard,
            roots: dedup_paths(recycle_bins),
            cleanup_dirs: true,
        },
        CategoryDef {
            id: "downloads_old",
            title: "下载文件夹",
            description: "清理超过30天的下载文件",
            kind: CategoryKind::DownloadsOlderThan { days: 30 },
            roots: dedup_paths(download_root),
            cleanup_dirs: false,
        },
        CategoryDef {
            id: "system_cache",
            title: "系统缓存",
            description: "Windows 更新和系统缓存文件",
            kind: CategoryKind::Standard,
            roots: dedup_paths(vec![
                system_root
                    .join("SoftwareDistribution")
                    .join("Download"),
                system_root
                    .join("SoftwareDistribution")
                    .join("DeliveryOptimization")
                    .join("Cache"),
            ]),
            cleanup_dirs: true,
        },
        CategoryDef {
            id: "browser_cache",
            title: "浏览器缓存",
            description: "清理浏览器缓存和 Cookie",
            kind: CategoryKind::Standard,
            roots: dedup_paths(browser_paths),
            cleanup_dirs: true,
        },
        CategoryDef {
            id: "system_logs",
            title: "系统日志",
            description: "Windows 事件日志和应用日志",
            kind: CategoryKind::Standard,
            roots: dedup_paths(vec![
                system_root.join("Logs"),
                system_root.join("System32").join("LogFiles"),
                system_root.join("Panther"),
            ]),
            cleanup_dirs: true,
        },
        CategoryDef {
            id: "windows_old",
            title: "旧 Windows 版本",
            description: "Windows 更新后保留的旧系统文件",
            kind: CategoryKind::Standard,
            roots: dedup_paths(vec![windows_old]),
            cleanup_dirs: true,
        },
    ]
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for path in paths {
        let key = normalize_path(&path);
        if seen.insert(key) {
            output.push(path);
        }
    }
    output
}

fn system_drive_mount() -> PathBuf {
    let drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    PathBuf::from(format!("{}\\", drive))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(icon) =
                    tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
                {
                    let _ = window.set_icon(icon);
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_disk_info,
            get_hibernation_info,
            set_hibernation_enabled,
            scan_cleanup_items,
            scan_large_items,
            list_category_items,
            clean_categories,
            clean_large_items
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
