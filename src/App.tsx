import { useEffect, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import "./App.css";

type DiskInfo = {
  mountPoint: string;
  totalBytes: number;
  freeBytes: number;
  usedBytes: number;
  usedPercent: number;
};

type CleanupCategory = {
  id: string;
  title: string;
  description: string;
  sizeBytes: number;
  fileCount: number;
};

type CleanupItem = {
  path: string;
  sizeBytes: number;
  modifiedMs?: number | null;
};

type LargeItem = {
  path: string;
  name: string;
  sizeBytes: number;
  isDir: boolean;
  suspicious: boolean;
  categoryId?: string | null;
};

type CategoryItems = {
  items: CleanupItem[];
  hasMore: boolean;
};

type CleanupResult = {
  deletedBytes: number;
  deletedCount: number;
  failed: { path: string; message: string }[];
};

const CATEGORY_ACCENTS: Record<string, string> = {
  temp_files: "#3b6cff",
  recycle_bin: "#13a672",
  downloads_old: "#f0932b",
  system_cache: "#2c84ff",
  browser_cache: "#2bb6c7",
  system_logs: "#6c7bff",
  windows_old: "#ff6b6b",
};

const formatBytes = (bytes: number) => {
  if (!bytes || bytes < 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, index);
  return `${value.toFixed(value >= 100 || index === 0 ? 0 : 2)} ${units[index]}`;
};

const formatDate = (ms?: number | null) => {
  if (!ms) return "未知时间";
  const formatter = new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
  return formatter.format(new Date(ms));
};

function App() {
  const [diskInfo, setDiskInfo] = useState<DiskInfo | null>(null);
  const [categories, setCategories] = useState<CleanupCategory[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanStatus, setScanStatus] = useState<string>("");
  const [cleaning, setCleaning] = useState(false);
  const [error, setError] = useState<string>("");
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [activeCategory, setActiveCategory] = useState<CleanupCategory | null>(
    null,
  );
  const [detailItems, setDetailItems] = useState<CleanupItem[]>([]);
  const [detailsLoading, setDetailsLoading] = useState(false);
  const [detailsHasMore, setDetailsHasMore] = useState(false);
  const [excludedPaths, setExcludedPaths] = useState<Record<string, string[]>>(
    {},
  );
  const [excludedSizes, setExcludedSizes] = useState<Record<string, number>>(
    {},
  );
  const [includedPaths, setIncludedPaths] = useState<Record<string, string[]>>(
    {},
  );
  const [includedSizes, setIncludedSizes] = useState<Record<string, number>>(
    {},
  );
  const [largeItems, setLargeItems] = useState<LargeItem[]>([]);
  const [largeScanning, setLargeScanning] = useState(false);
  const [largeSelectedPaths, setLargeSelectedPaths] = useState<string[]>([]);
  const [showSuspiciousOnly, setShowSuspiciousOnly] = useState(false);
  const [scanProgress, setScanProgress] = useState(0);

  useEffect(() => {
    invoke<DiskInfo>("get_disk_info")
      .then(setDiskInfo)
      .catch((err) => {
        setError(String(err));
      });
  }, []);

  const scanActive = scanning || largeScanning;

  useEffect(() => {
    if (!scanActive) {
      setScanProgress(0);
      return;
    }
    setScanProgress(12);
    const interval = setInterval(() => {
      setScanProgress((prev) => {
        const next = prev + 6 + Math.random() * 8;
        return next >= 90 ? 90 : next;
      });
    }, 420);
    return () => clearInterval(interval);
  }, [scanActive]);

  const standaloneSizeByPath = useMemo(() => {
    const map = new Map<string, number>();
    for (const item of largeItems) {
      if (!item.categoryId) {
        map.set(item.path, item.sizeBytes);
      }
    }
    return map;
  }, [largeItems]);

  const standaloneSelectedBytes = useMemo(() => {
    return largeSelectedPaths.reduce((sum, path) => {
      return sum + (standaloneSizeByPath.get(path) ?? 0);
    }, 0);
  }, [largeSelectedPaths, standaloneSizeByPath]);

  const categorySelectedBytes = useMemo(() => {
    return categories.reduce((sum, category) => {
      if (selectedIds.includes(category.id)) {
        const excluded = excludedSizes[category.id] ?? 0;
        return sum + Math.max(0, category.sizeBytes - excluded);
      }
      const included = includedSizes[category.id] ?? 0;
      return sum + included;
    }, 0);
  }, [selectedIds, categories, excludedSizes, includedSizes]);

  const selectedSize = useMemo(() => {
    return categorySelectedBytes + standaloneSelectedBytes;
  }, [categorySelectedBytes, standaloneSelectedBytes]);

  const quickSelectionCount = useMemo(() => {
    const set = new Set(selectedIds);
    for (const [id, paths] of Object.entries(includedPaths)) {
      if (paths.length > 0) {
        set.add(id);
      }
    }
    return set.size;
  }, [selectedIds, includedPaths]);

  const selectedEntryCount = useMemo(() => {
    const set = new Set(selectedIds);
    for (const [id, paths] of Object.entries(includedPaths)) {
      if (paths.length > 0) {
        set.add(id);
      }
    }
    return set.size + largeSelectedPaths.length;
  }, [selectedIds, includedPaths, largeSelectedPaths]);

  const hasSelection = selectedEntryCount > 0;

  const allSelected =
    categories.length > 0 && selectedIds.length === categories.length;

  const sortedCategories = useMemo(() => {
    return [...categories].sort((a, b) => b.sizeBytes - a.sizeBytes);
  }, [categories]);

  const excludedLookup = useMemo(() => {
    const map: Record<string, Set<string>> = {};
    for (const [id, paths] of Object.entries(excludedPaths)) {
      map[id] = new Set(paths);
    }
    return map;
  }, [excludedPaths]);

  const includedLookup = useMemo(() => {
    const map: Record<string, Set<string>> = {};
    for (const [id, paths] of Object.entries(includedPaths)) {
      map[id] = new Set(paths);
    }
    return map;
  }, [includedPaths]);

  const excludedCount = useMemo(() => {
    if (!activeCategory) return 0;
    return excludedPaths[activeCategory.id]?.length ?? 0;
  }, [activeCategory, excludedPaths]);

  const includedCount = useMemo(() => {
    if (!activeCategory) return 0;
    return includedPaths[activeCategory.id]?.length ?? 0;
  }, [activeCategory, includedPaths]);

  const handleScan = async () => {
    setScanning(true);
    setError("");
    setScanStatus("正在分析磁盘…");
    try {
      const [disk, items] = await Promise.all([
        invoke<DiskInfo>("get_disk_info"),
        invoke<CleanupCategory[]>("scan_cleanup_items"),
      ]);
      const sorted = [...items].sort((a, b) => b.sizeBytes - a.sizeBytes);
      setDiskInfo(disk);
      setCategories(sorted);
      setSelectedIds([]);
      setExcludedPaths({});
      setExcludedSizes({});
      setIncludedPaths({});
      setIncludedSizes({});
      setScanStatus(`扫描完成，发现 ${sorted.length} 项可清理`);
    } catch (err) {
      setError(String(err));
      setScanStatus("扫描失败，请稍后重试");
    } finally {
      setScanning(false);
    }
  };

  const handleLargeScan = async () => {
    setLargeScanning(true);
    setError("");
    setScanStatus("正在扫描大文件…");
    try {
      const items = await invoke<LargeItem[]>("scan_large_items");
      setLargeItems(items);
      const standalonePaths = new Set(
        items.filter((item) => !item.categoryId).map((item) => item.path),
      );
      setLargeSelectedPaths((prev) =>
        prev.filter((path) => standalonePaths.has(path)),
      );
      setScanStatus(`扫描完成，发现 ${items.length} 项大文件/文件夹`);
    } catch (err) {
      setError(String(err));
      setScanStatus("扫描失败，请稍后重试");
    } finally {
      setLargeScanning(false);
    }
  };

  const toggleCategory = (id: string) => {
    setSelectedIds((prev) =>
      prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id],
    );
    setIncludedPaths((prev) => {
      if (!prev[id]) return prev;
      const { [id]: _, ...rest } = prev;
      return rest;
    });
    setIncludedSizes((prev) => {
      if (prev[id] === undefined) return prev;
      const { [id]: _, ...rest } = prev;
      return rest;
    });
  };

  const toggleSelectAll = () => {
    if (allSelected) {
      setSelectedIds([]);
      return;
    }
    setSelectedIds(categories.map((item) => item.id));
    setIncludedPaths({});
    setIncludedSizes({});
  };

  const openDetails = async (category: CleanupCategory) => {
    setActiveCategory(category);
    setDetailsOpen(true);
    setDetailsLoading(true);
    try {
      const response = await invoke<CategoryItems>("list_category_items", {
        id: category.id,
        limit: 300,
      });
      setDetailItems(response.items);
      setDetailsHasMore(response.hasMore);
    } catch (err) {
      setDetailItems([]);
      setDetailsHasMore(false);
      setError(String(err));
    } finally {
      setDetailsLoading(false);
    }
  };

  const closeDetails = () => {
    setDetailsOpen(false);
    setActiveCategory(null);
    setDetailItems([]);
    setDetailsHasMore(false);
  };

  const handleReveal = async (path: string) => {
    try {
      await revealItemInDir(path);
    } catch (err) {
      setError(String(err));
    }
  };

  const handleDetailToggle = (
    categoryId: string,
    item: CleanupItem,
    nextChecked: boolean,
    parentSelected: boolean,
    isExcluded: boolean,
    isIncluded: boolean,
  ) => {
    if (parentSelected) {
      setExcludedPaths((prev) => {
        const set = new Set(prev[categoryId] ?? []);
        if (nextChecked) {
          set.delete(item.path);
        } else {
          set.add(item.path);
        }
        setExcludedSizes((sizes) => {
          const current = sizes[categoryId] ?? 0;
          const delta =
            nextChecked && isExcluded
              ? -item.sizeBytes
              : !nextChecked && !isExcluded
                ? item.sizeBytes
                : 0;
          const nextValue = Math.max(0, current + delta);
          return { ...sizes, [categoryId]: nextValue };
        });
        return { ...prev, [categoryId]: Array.from(set) };
      });
      return;
    }

    setIncludedPaths((prev) => {
      const set = new Set(prev[categoryId] ?? []);
      if (nextChecked) {
        set.add(item.path);
      } else {
        set.delete(item.path);
      }
      setIncludedSizes((sizes) => {
        const current = sizes[categoryId] ?? 0;
        const delta =
          nextChecked && !isIncluded
            ? item.sizeBytes
            : !nextChecked && isIncluded
              ? -item.sizeBytes
              : 0;
        const nextValue = Math.max(0, current + delta);
        return { ...sizes, [categoryId]: nextValue };
      });
      return { ...prev, [categoryId]: Array.from(set) };
    });
  };

  const isLargeItemChecked = (item: LargeItem) => {
    if (item.categoryId) {
      const parentSelected = selectedIds.includes(item.categoryId);
      const excludedSet = excludedLookup[item.categoryId];
      const includedSet = includedLookup[item.categoryId];
      const isExcluded = excludedSet?.has(item.path) ?? false;
      const isIncluded = includedSet?.has(item.path) ?? false;
      return parentSelected ? !isExcluded : isIncluded;
    }
    return largeSelectedPaths.includes(item.path);
  };

  const handleLargeToggle = (item: LargeItem, nextChecked: boolean) => {
    if (item.categoryId) {
      const parentSelected = selectedIds.includes(item.categoryId);
      const excludedSet = excludedLookup[item.categoryId];
      const includedSet = includedLookup[item.categoryId];
      handleDetailToggle(
        item.categoryId,
        { path: item.path, sizeBytes: item.sizeBytes },
        nextChecked,
        parentSelected,
        excludedSet?.has(item.path) ?? false,
        includedSet?.has(item.path) ?? false,
      );
      return;
    }
    setLargeSelectedPaths((prev) => {
      const set = new Set(prev);
      if (nextChecked) {
        set.add(item.path);
      } else {
        set.delete(item.path);
      }
      return Array.from(set);
    });
  };

  const handleClean = async () => {
    if (!hasSelection || cleaning) return;
    setCleaning(true);
    setScanStatus("正在清理中，请保持应用打开…");
    setError("");
    const hadLargeSelection = largeSelectedPaths.length > 0;
    const categoryStats = categories.reduce<
      Record<string, { sizeBytes: number; fileCount: number }>
    >((acc, category) => {
      acc[category.id] = {
        sizeBytes: category.sizeBytes,
        fileCount: category.fileCount,
      };
      return acc;
    }, {});
    try {
      const categoryResult = await invoke<CleanupResult>("clean_categories", {
        request: {
          ids: selectedIds,
          excludedPaths,
          includedPaths,
          categoryStats,
        },
      });
      let totalDeletedBytes = categoryResult.deletedBytes;
      let totalDeletedCount = categoryResult.deletedCount;
      let failed = [...categoryResult.failed];

      if (hadLargeSelection) {
        const largeResult = await invoke<CleanupResult>("clean_large_items", {
          paths: largeSelectedPaths,
        });
        totalDeletedBytes += largeResult.deletedBytes;
        totalDeletedCount += largeResult.deletedCount;
        failed = [...failed, ...largeResult.failed];
      }

      const summary = `清理完成，删除 ${totalDeletedCount} 项，释放 ${formatBytes(
        totalDeletedBytes,
      )}`;
      setScanStatus(
        failed.length
          ? `${summary}，但有 ${failed.length} 项未能删除`
          : summary,
      );
      const updated = await invoke<CleanupCategory[]>("scan_cleanup_items");
      setCategories(updated);
      setSelectedIds([]);
      setIncludedPaths({});
      setIncludedSizes({});
      setLargeSelectedPaths([]);

      if (hadLargeSelection) {
        const refreshedLarge = await invoke<LargeItem[]>("scan_large_items");
        setLargeItems(refreshedLarge);
      }
    } catch (err) {
      setError(String(err));
      setScanStatus("清理失败，请检查权限后重试");
    } finally {
      setCleaning(false);
    }
  };

  const toggleSelectAllLarge = () => {
    if (filteredLargeItems.length === 0) return;
    const nextChecked = !allLargeSelected;
    for (const item of filteredLargeItems) {
      const isChecked = isLargeItemChecked(item);
      if (isChecked === nextChecked) continue;
      handleLargeToggle(item, nextChecked);
    }
  };

  const handleCancel = () => {
    setSelectedIds([]);
    setIncludedPaths({});
    setIncludedSizes({});
    setLargeSelectedPaths([]);
    setScanStatus("");
  };

  const excludedSet = useMemo(() => {
    if (!activeCategory) return new Set<string>();
    return new Set(excludedPaths[activeCategory.id] ?? []);
  }, [activeCategory, excludedPaths]);

  const includedSet = useMemo(() => {
    if (!activeCategory) return new Set<string>();
    return new Set(includedPaths[activeCategory.id] ?? []);
  }, [activeCategory, includedPaths]);

  const parentSelected = activeCategory
    ? selectedIds.includes(activeCategory.id)
    : false;

  const filteredLargeItems = showSuspiciousOnly
    ? largeItems.filter((item) => item.suspicious)
    : largeItems;

  const largeSelectedBytes = largeItems.reduce((sum, item) => {
    return isLargeItemChecked(item) ? sum + item.sizeBytes : sum;
  }, 0);

  const allLargeSelected =
    filteredLargeItems.length > 0 &&
    filteredLargeItems.every((item) => isLargeItemChecked(item));

  const usedPercent = diskInfo?.usedPercent ?? 0;

  return (
    <div className="app">
      <header className="hero">
        <div className="hero-icon">
          <svg viewBox="0 0 48 48" aria-hidden>
            <rect x="6" y="10" width="36" height="28" rx="8" />
            <path d="M14 20h20M16 28h4" />
          </svg>
        </div>
        <div className="hero-text">
          <h1>C 盘清理工具</h1>
          <p>扫描并清理不必要的文件，释放磁盘空间</p>
        </div>
      </header>

      <section className="card disk-card">
        <div className="disk-header">
          <div>
            <h2>本地磁盘 (C:)</h2>
            <p>
              可用空间{" "}
              <strong>{formatBytes(diskInfo?.freeBytes ?? 0)}</strong> / 总容量{" "}
              <strong>{formatBytes(diskInfo?.totalBytes ?? 0)}</strong>
            </p>
          </div>
        </div>
        <div className="progress">
          <div className="progress-track">
            <div
              className="progress-fill"
              style={{ width: `${Math.min(100, usedPercent)}%` }}
            />
          </div>
          <div className="progress-meta">
            已使用 {formatBytes(diskInfo?.usedBytes ?? 0)} (
            {usedPercent.toFixed(1)}%)
          </div>
        </div>
      </section>

      <section className="cleanup-section">
        {scanActive && (
          <div className="card scan-card">
            <div className="scan-header">
              <div className="scan-title">
                {scanning ? "正在扫描可清理文件…" : "正在扫描大文件…"}
              </div>
              <div className="scan-percent">
                {Math.min(99, Math.round(scanProgress))}%
              </div>
            </div>
            <div className="scan-progress">
              <div className="scan-progress-track">
                <div
                  className="scan-progress-fill"
                  style={{ width: `${Math.min(99, scanProgress)}%` }}
                />
              </div>
            </div>
          </div>
        )}

        <div className="cleanup-grid">
          <div className="card panel-card">
            <div className="panel-top">
              <div className="panel-title">
                <span className="panel-icon quick">
                  <svg viewBox="0 0 24 24" aria-hidden>
                    <rect x="4" y="6" width="16" height="12" rx="3" />
                    <path d="M7 11h10M8 15h4" />
                  </svg>
                </span>
                <div>
                  <h3>快速清理</h3>
                  <p>扫描常见缓存与临时文件</p>
                </div>
              </div>
              <button
                className="ghost-button"
                type="button"
                onClick={handleScan}
                disabled={scanning}
              >
                {scanning ? "扫描中" : "扫描"}
              </button>
            </div>

            {categories.length === 0 ? (
              <div className="panel-empty">
                <div className="panel-illustration">
                  <svg viewBox="0 0 64 64" aria-hidden>
                    <rect x="12" y="14" width="40" height="36" rx="10" />
                    <path d="M20 30h24M24 38h8" />
                  </svg>
                </div>
                <p>点击扫描按钮开始分析可清理的文件</p>
              </div>
            ) : (
              <>
                <div className="panel-subheader">
                  <span>
                    已选择 {quickSelectionCount} 项，可释放{" "}
                    {formatBytes(categorySelectedBytes)}
                  </span>
                  <button
                    className="text-button"
                    type="button"
                    onClick={toggleSelectAll}
                  >
                    {allSelected ? "取消全选" : "全选"}
                  </button>
                </div>
                <div className="cleanup-list">
                  {sortedCategories.map((category, index) => {
                    const selected = selectedIds.includes(category.id);
                    const accent =
                      CATEGORY_ACCENTS[category.id] ?? "var(--accent)";
                    return (
                      <div
                        key={category.id}
                        className={`cleanup-item ${selected ? "selected" : ""}`}
                        style={
                          {
                            "--accent": accent,
                            "--delay": `${index * 80}ms`,
                          } as CSSProperties
                        }
                      >
                        <label className="checkbox">
                          <input
                            type="checkbox"
                            checked={selected}
                            onChange={() => toggleCategory(category.id)}
                          />
                          <span className="checkbox-mark" aria-hidden />
                        </label>
                        <div className="item-icon">
                          <CategoryIcon id={category.id} />
                        </div>
                        <div className="item-body">
                          <div className="item-title">{category.title}</div>
                          <div className="item-desc">{category.description}</div>
                          <div className="item-meta">
                            {category.fileCount} 项可清理
                          </div>
                        </div>
                        <div className="item-actions">
                          <button
                            className="link-button"
                            type="button"
                            onClick={() => openDetails(category)}
                          >
                            查看详情
                          </button>
                          <div className="item-size">
                            {formatBytes(category.sizeBytes)}
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>

          <div className="card panel-card">
            <div className="panel-top">
              <div className="panel-title">
                <span className="panel-icon large">
                  <svg viewBox="0 0 24 24" aria-hidden>
                    <circle cx="11" cy="11" r="6.5" />
                    <path d="M16.5 16.5L20 20" />
                  </svg>
                </span>
                <div>
                  <h3>大文件扫描</h3>
                  <p>快速定位占用空间的文件/文件夹</p>
                </div>
              </div>
              <button
                className="ghost-button"
                type="button"
                onClick={handleLargeScan}
                disabled={largeScanning}
              >
                {largeScanning ? "扫描中" : "扫描"}
              </button>
            </div>

            {largeItems.length === 0 ? (
              <div className="panel-empty">
                <div className="panel-illustration">
                  <svg viewBox="0 0 64 64" aria-hidden>
                    <circle cx="30" cy="30" r="16" />
                    <path d="M44 44l12 12" />
                  </svg>
                </div>
                <p>
                  点击扫描按钮查看占用大量空间的文件
                  <br />
                  自动识别 log、cache、temp 等可疑文件夹
                </p>
              </div>
            ) : (
              <>
                <div className="panel-subheader large-subheader">
                  <span>可释放 {formatBytes(largeSelectedBytes)}</span>
                  <div className="panel-actions">
                    <button
                      className={`chip ${showSuspiciousOnly ? "active" : ""}`}
                      type="button"
                      onClick={() =>
                        setShowSuspiciousOnly((prev) => !prev)
                      }
                    >
                      可疑项
                    </button>
                    <button
                      className="text-button"
                      type="button"
                      onClick={toggleSelectAllLarge}
                    >
                      {allLargeSelected ? "取消全选" : "全选"}
                    </button>
                  </div>
                </div>
                <div className="large-list">
                  {filteredLargeItems.map((item) => {
                    const checked = isLargeItemChecked(item);
                    return (
                      <div
                        key={item.path}
                        className={`large-item ${
                          checked ? "selected" : ""
                        } ${item.suspicious ? "suspicious" : ""}`}
                      >
                        <label className="checkbox">
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => handleLargeToggle(item, !checked)}
                          />
                          <span className="checkbox-mark" aria-hidden />
                        </label>
                        <div className="large-icon">
                          <LargeItemIcon isDir={item.isDir} />
                        </div>
                        <div className="large-body">
                          <div className="large-title">
                            <span>{item.name}</span>
                            {item.suspicious && (
                              <span className="tag">可疑</span>
                            )}
                          </div>
                          <div className="large-path">{item.path}</div>
                        </div>
                        <div className="large-size">
                          {formatBytes(item.sizeBytes)}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>
        </div>

        {(categories.length > 0 ||
          largeItems.length > 0 ||
          hasSelection ||
          scanStatus ||
          error) && (
          <div className="action-bar">
            <div>
              <div className="action-summary">
                已选择 {selectedEntryCount} 项，可释放{" "}
                {formatBytes(selectedSize)}
              </div>
              {scanStatus && <div className="action-status">{scanStatus}</div>}
              {error && <div className="action-error">{error}</div>}
            </div>
            <div className="action-buttons">
              <button
                className="secondary-button"
                type="button"
                onClick={handleCancel}
                disabled={cleaning}
              >
                取消
              </button>
              <button
                className="primary-button"
                type="button"
                onClick={handleClean}
                disabled={!hasSelection || cleaning}
              >
                {cleaning ? "清理中…" : "开始清理"}
              </button>
            </div>
          </div>
        )}
      </section>

      {detailsOpen && activeCategory && (
        <div className="details-overlay" role="dialog" aria-modal="true">
          <div className="details-card">
            <div className="details-header">
              <div>
                <h3>{activeCategory.title}</h3>
                <p>
                  {activeCategory.description} ·{" "}
                  {parentSelected
                    ? `已排除 ${excludedCount} 项`
                    : `已选择 ${includedCount} 项`}
                </p>
              </div>
              <button
                className="ghost-button"
                type="button"
                onClick={closeDetails}
              >
                关闭
              </button>
            </div>

            {detailsLoading ? (
              <div className="details-loading">正在加载内容…</div>
            ) : detailItems.length === 0 ? (
              <div className="details-empty">暂无可展示的文件条目</div>
            ) : (
              <div className="details-list">
                {detailItems.map((item) => {
                  const isExcluded = excludedSet.has(item.path);
                  const isIncluded = includedSet.has(item.path);
                  const isChecked = parentSelected ? !isExcluded : isIncluded;
                  return (
                    <div key={item.path} className="details-item">
                      <label className="details-checkbox">
                        <input
                          type="checkbox"
                          checked={isChecked}
                          onChange={() =>
                            handleDetailToggle(
                              activeCategory.id,
                              item,
                              !isChecked,
                              parentSelected,
                              isExcluded,
                              isIncluded,
                            )
                          }
                        />
                        <span>清理</span>
                      </label>
                      <div className="details-info">
                        <div className="details-path">{item.path}</div>
                        <div className="details-meta">
                          {formatBytes(item.sizeBytes)} · {formatDate(item.modifiedMs)}
                        </div>
                      </div>
                      <div className="details-actions">
                        <button
                          className="link-button details-open"
                          type="button"
                          onClick={() => handleReveal(item.path)}
                        >
                          打开位置
                        </button>
                      </div>
                    </div>
                  );
                })}
                {detailsHasMore && (
                  <div className="details-more">
                    仅显示部分文件，更多内容请在资源管理器中查看。
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function CategoryIcon({ id }: { id: string }) {
  switch (id) {
    case "recycle_bin":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M8 7h8m-6 3v6m4-6v6M6 7h12l-1 13a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2L6 7zM9 4h6l1 3H8l1-3z" />
        </svg>
      );
    case "downloads_old":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M12 3v11m0 0l4-4m-4 4l-4-4M5 19h14" />
        </svg>
      );
    case "system_cache":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M12 4a8 8 0 1 1-7.4 5M4 4v5h5" />
        </svg>
      );
    case "browser_cache":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <circle cx="12" cy="12" r="8" />
          <path d="M12 4a8 8 0 0 1 7.4 5H6.6" />
          <path d="M12 20a8 8 0 0 1-7.4-5h12.8" />
        </svg>
      );
    case "system_logs":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M6 4h9l3 3v13H6z" />
          <path d="M9 11h6M9 15h6" />
        </svg>
      );
    case "windows_old":
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M4 5h9v14H4zM15 5h5v5h-5zM15 12h5v7h-5z" />
        </svg>
      );
    case "temp_files":
    default:
      return (
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M6 4h7l5 5v11H6z" />
          <path d="M9 13h6M9 17h4" />
        </svg>
      );
  }
}

function LargeItemIcon({ isDir }: { isDir: boolean }) {
  if (isDir) {
    return (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" aria-hidden>
      <path d="M6 4h7l5 5v11H6z" />
      <path d="M9 13h6M9 17h4" />
    </svg>
  );
}

export default App;
