use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use sysinfo::Disks;
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
struct CleanRequest {
    ids: Vec<String>,
    #[serde(default)]
    excluded_paths: HashMap<String, Vec<String>>,
    #[serde(default)]
    included_paths: HashMap<String, Vec<String>>,
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
async fn scan_cleanup_items() -> Result<Vec<CleanupCategory>, String> {
    ensure_windows()?;
    tauri::async_runtime::spawn_blocking(move || scan_cleanup_items_sync())
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
        let result = clean_category(def, &excluded);
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

fn clean_category(def: &CategoryDef, excluded: &HashSet<String>) -> CleanupResult {
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
        .invoke_handler(tauri::generate_handler![
            get_disk_info,
            scan_cleanup_items,
            list_category_items,
            clean_categories
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
