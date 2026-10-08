//! 截图 blob 存储（文件系统）+ 保留策略。
//!
//! 落盘约定：
//! **数据库只存路径与哈希，图片落文件系统** —— 两者混在一起会让磁盘与 DB
//! 一起失控，清理策略也无从下手。
//!
//! 两条关键性质：
//! 1. **原子写入**：写临时文件 → `rename`。崩溃不会留下半张图。
//! 2. **内容寻址**：文件名就是内容哈希。同一份内容在同一天只存一份，
//!    代价是天然去重，收益是「同一张截图被重复分析」在存储层就没有土壤。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Utc};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub captured_at: Timestamp,
    pub display_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobId(pub String);

impl BlobId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一块已落盘的 blob（主图或缩略图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlob {
    pub blob_id: BlobId,
    pub path: PathBuf,
    /// 相对 store 根目录的路径，写进数据库的就是它
    pub relative_path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImage {
    pub blob_id: BlobId,
    pub path: PathBuf,
    pub relative_path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub content_hash: String,
    pub thumbnail: Option<StoredBlob>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlobStats {
    pub blob_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// 0 = 永久保留
    pub screenshots_days: u32,
    /// 0 = 不限制。
    ///
    /// 0 不能当成「上限就是 0」：一次配置手滑就会删光所有截图。
    /// 保留策略是**不可逆**的，缺省语义必须是「不做」而不是「全做」。
    pub max_total_bytes: u64,
    /// 0 = 不限制
    pub max_blob_count: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            // 与 mc-config 的 capture.retention_days / storage 默认值一致
            screenshots_days: 7,
            max_total_bytes: 10 * 1024 * 1024 * 1024,
            max_blob_count: 200_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetentionReport {
    pub deleted: u64,
    pub freed_bytes: u64,
    pub kept: u64,
}

pub trait BlobStore: Send + Sync {
    fn put_image(&self, image: &RgbImage, meta: &ImageMeta) -> Result<StoredImage, AppError>;
    fn get(&self, id: &BlobId) -> Result<Vec<u8>, AppError>;
    fn stats(&self) -> Result<BlobStats, AppError>;
}

#[derive(Debug, Clone)]
pub struct FileSystemBlobStore {
    root: PathBuf,
    format: ImageFormat,
    quality: u8,
    thumbnail_width: u32,
}

const THUMBNAIL_DIR: &str = "thumbnails";
const SCREENSHOT_DIR: &str = "screenshots";
const DEFAULT_THUMBNAIL_WIDTH: u32 = 320;

impl FileSystemBlobStore {
    pub fn new(root: PathBuf, format: ImageFormat) -> Result<Self, AppError> {
        std::fs::create_dir_all(&root).map_err(|e| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法创建 blob 根目录 {}: {e}", root.display()),
            )
        })?;
        Ok(Self {
            root,
            format,
            quality: 85,
            thumbnail_width: DEFAULT_THUMBNAIL_WIDTH,
        })
    }

    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality.clamp(1, 100);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// 按时间清理过期 / 超额 blob。
    pub fn enforce_retention(
        &self,
        now: Timestamp,
        policy: &RetentionPolicy,
    ) -> Result<RetentionReport, AppError> {
        let mut entries = self.list_entries()?;
        let mut report = RetentionReport::default();

        // 1) 按天数清理
        if policy.screenshots_days > 0 {
            let cutoff = now.minus_millis(policy.screenshots_days as i64 * 86_400_000);
            let mut survivors = Vec::new();
            for (path, captured_at) in entries {
                if captured_at < cutoff {
                    report.freed_bytes += delete_file(&path)?;
                    report.deleted += 1;
                } else {
                    survivors.push((path, captured_at));
                }
            }
            entries = survivors;
        }

        // 2) 按总字节清理（从最旧开始）；上限为 0 表示不限制
        entries.sort_by_key(|(_, captured_at)| *captured_at);
        let mut total: u64 = entries.iter().map(|(p, _)| file_size(p)).sum();
        while policy.max_total_bytes > 0 && total > policy.max_total_bytes && !entries.is_empty() {
            let (path, _) = entries.remove(0);
            let freed = delete_file(&path)?;
            report.freed_bytes += freed;
            report.deleted += 1;
            total = total.saturating_sub(freed);
        }

        // 3) 按条目数清理
        if policy.max_blob_count > 0 {
            while entries.len() as u64 > policy.max_blob_count && !entries.is_empty() {
                let (path, _) = entries.remove(0);
                let freed = delete_file(&path)?;
                report.freed_bytes += freed;
                report.deleted += 1;
            }
        }

        report.kept = entries.len() as u64;
        self.prune_empty_dirs()?;
        Ok(report)
    }

    pub fn stats(&self) -> Result<BlobStats, AppError> {
        let entries = self.list_entries()?;
        Ok(BlobStats {
            blob_count: entries.len() as u64,
            total_bytes: entries.iter().map(|(p, _)| file_size(p)).sum(),
        })
    }

    /// 按**相对路径**读取（例如 `screenshots/2026/09/30/x.png`）。
    ///
    /// 只接受 store 内两种受控前缀，并拒绝 `..` / 绝对路径 ——
    /// 路径校验放在这里而不是调用方，因为它是安全属性，漏一处就出洞。
    pub fn read_relative(&self, relative: &str) -> Result<Vec<u8>, AppError> {
        let path = self.resolve_relative(relative)?;
        std::fs::read(&path).map_err(|e| {
            AppError::new(
                ErrorCode::StorageBlobMissing,
                format!("读取截图失败（文件可能已被保留策略清理）: {e}"),
            )
        })
    }

    pub fn relative_exists(&self, relative: &str) -> bool {
        self.resolve_relative(relative)
            .map(|p| p.is_file())
            .unwrap_or(false)
    }

    /// 把相对路径解析成 store 内的绝对路径，并做安全校验。
    pub fn resolve_relative(&self, relative: &str) -> Result<PathBuf, AppError> {
        let reject = |reason: &str| {
            Err(AppError::new(
                ErrorCode::StorageInvalidBlobPath,
                format!("非法 blob 路径 `{relative}`：{reason}"),
            ))
        };

        if relative.is_empty() {
            return reject("路径为空");
        }
        if relative.starts_with('/') || relative.contains(':') || relative.contains('\\') {
            return reject("不允许绝对路径或盘符");
        }
        if relative.split(['/', '\\']).any(|part| part == "..") {
            return reject("不允许路径穿越");
        }

        let normalized = relative.replace('\\', "/");
        if !(normalized.starts_with("screenshots/") || normalized.starts_with("thumbnails/")) {
            return reject("只允许 screenshots/ 与 thumbnails/ 前缀");
        }

        Ok(self.root.join(normalized))
    }

    pub fn get(&self, id: &BlobId) -> Result<Vec<u8>, AppError> {
        let path = self.find_by_id(id)?.ok_or_else(|| {
            AppError::new(
                ErrorCode::StorageBlobMissing,
                format!("blob {} 不存在（文件可能已被清理或手动删除）", id.as_str()),
            )
        })?;
        std::fs::read(&path).map_err(|e| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("读取 blob {} 失败: {e}", path.display()),
            )
        })
    }

    // ---------------- 内部 ----------------

    fn find_by_id(&self, id: &BlobId) -> Result<Option<PathBuf>, AppError> {
        for (path, _) in self.list_entries()? {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem == id.as_str() {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }

    fn encode(&self, image: &RgbImage) -> Result<Vec<u8>, AppError> {
        let mut buffer = Vec::new();
        let (width, height) = (image.width(), image.height());
        let bytes = image.as_raw();

        let result =
            match self.format {
                ImageFormat::Png => PngEncoder::new(&mut buffer).write_image(
                    bytes,
                    width,
                    height,
                    ExtendedColorType::Rgb8,
                ),
                ImageFormat::Jpeg => JpegEncoder::new_with_quality(&mut buffer, self.quality)
                    .encode(bytes, width, height, ExtendedColorType::Rgb8),
            };

        result.map_err(|e| {
            AppError::new(ErrorCode::StorageUnavailable, format!("图像编码失败: {e}"))
        })?;

        Ok(buffer)
    }

    fn relative_for(&self, kind: &str, at: Timestamp, hash: &str) -> String {
        let (year, month, day) = utc_ymd(at);
        format!(
            "{kind}/{year}/{month:02}/{day:02}/{hash}.{}",
            self.format.extension()
        )
    }

    /// 原子写入：先写同目录下的临时文件，`fsync` 后 `rename`。
    fn write_atomic(&self, relative: &str, bytes: &[u8]) -> Result<PathBuf, AppError> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(map_write_error)?;
        }

        let temp = path.with_extension(format!("{}.part", self.format.extension()));
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temp).map_err(map_write_error)?;
            file.write_all(bytes).map_err(map_write_error)?;
            file.sync_all().map_err(map_write_error)?;
        }

        std::fs::rename(&temp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            map_write_error(e)
        })?;

        Ok(path)
    }

    /// 当前存在的全部 blob 的**相对路径**（截图 + 缩略图）。
    ///
    /// 保留策略要判断「数据库里的引用还有没有对应文件」，
    /// 用一次目录遍历拿到全集，比逐条 `stat` 快得多。
    pub fn list_relative(&self) -> Result<Vec<String>, AppError> {
        let mut result = Vec::new();
        for (path, _) in self.list_entries()? {
            if let Ok(relative) = path.strip_prefix(&self.root) {
                result.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
        result.sort();
        Ok(result)
    }

    fn list_entries(&self) -> Result<Vec<(PathBuf, Timestamp)>, AppError> {
        let mut entries = Vec::new();
        for kind in [SCREENSHOT_DIR, THUMBNAIL_DIR] {
            let base = self.root.join(kind);
            if !base.is_dir() {
                continue;
            }
            collect_files(&base, &mut entries);
        }
        Ok(entries)
    }

    fn prune_empty_dirs(&self) -> Result<(), AppError> {
        for kind in [SCREENSHOT_DIR, THUMBNAIL_DIR] {
            prune(kind, &self.root.join(kind));
        }
        Ok(())
    }
}

impl BlobStore for FileSystemBlobStore {
    fn put_image(&self, image: &RgbImage, meta: &ImageMeta) -> Result<StoredImage, AppError> {
        let bytes = self.encode(image)?;
        let hash = blake3::hash(&bytes).to_hex().to_string();

        let relative = self.relative_for(SCREENSHOT_DIR, meta.captured_at, &hash);
        let path = self.write_atomic(&relative, &bytes)?;

        let thumbnail = self.write_thumbnail(image, meta, &hash)?;

        Ok(StoredImage {
            blob_id: BlobId(hash.clone()),
            path,
            relative_path: relative,
            bytes: bytes.len() as u64,
            width: image.width(),
            height: image.height(),
            content_hash: hash,
            thumbnail,
        })
    }

    fn get(&self, id: &BlobId) -> Result<Vec<u8>, AppError> {
        FileSystemBlobStore::get(self, id)
    }

    fn stats(&self) -> Result<BlobStats, AppError> {
        FileSystemBlobStore::stats(self)
    }
}

impl FileSystemBlobStore {
    fn write_thumbnail(
        &self,
        image: &RgbImage,
        meta: &ImageMeta,
        parent_hash: &str,
    ) -> Result<Option<StoredBlob>, AppError> {
        if image.width() <= self.thumbnail_width {
            return Ok(None);
        }
        let height = ((image.height() as u64 * self.thumbnail_width as u64)
            / image.width().max(1) as u64)
            .max(1) as u32;
        let small = image::imageops::resize(
            image,
            self.thumbnail_width,
            height,
            image::imageops::FilterType::Triangle,
        );
        let bytes = self.encode(&small)?;
        // 用「父哈希 + 尺寸」派生缩略图名，避免与主图同哈希导致同一路径冲突
        let thumb_hash = format!("{}-thumb", &parent_hash[..parent_hash.len().min(16)]);

        let relative = self.relative_for(THUMBNAIL_DIR, meta.captured_at, &thumb_hash);
        let path = self.write_atomic(&relative, &bytes)?;

        Ok(Some(StoredBlob {
            blob_id: BlobId(thumb_hash.clone()),
            path,
            relative_path: relative,
            bytes: bytes.len() as u64,
            width: small.width(),
            height: small.height(),
            content_hash: blake3::hash(&bytes).to_hex().to_string(),
        }))
    }
}

fn utc_ymd(at: Timestamp) -> (i32, u32, u32) {
    let dt: DateTime<Utc> = DateTime::from_timestamp_millis(at.as_millis())
        .unwrap_or_else(|| DateTime::from_timestamp_millis(0).expect("epoch"));
    (dt.year(), dt.month(), dt.day())
}

fn map_write_error(error: std::io::Error) -> AppError {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    let code = if lower.contains("space")
        || lower.contains("enospc")
        || error.raw_os_error() == Some(28)
    {
        ErrorCode::StorageDiskFull
    } else {
        ErrorCode::StorageUnavailable
    };
    AppError::new(code, format!("写入 blob 失败: {text}"))
}

fn delete_file(path: &Path) -> Result<u64, AppError> {
    let size = file_size(path);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(size),
        // 已经被删掉了不算失败（保留策略要幂等）
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(AppError::new(
            ErrorCode::StorageUnavailable,
            format!("删除 blob {} 失败: {e}", path.display()),
        )),
    }
}

/// 从 `<kind>/YYYY/MM/DD/<hash>.<ext>` 路径解析采集日期。
///
/// **不能用文件 mtime**：文件总是「现在」创建的，mtime 无法反映截图何时拍的。
/// 日期目录布局存在的意义就是把采集日期固化在路径里，保留策略必须读它。
/// 解析失败时退回 mtime，保证不会因为一次异常命名而误删。
fn captured_at_from_path(path: &Path) -> Timestamp {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    if parts.len() >= 4 {
        let (y, m, d) = (
            parts[parts.len() - 4].parse::<i32>(),
            parts[parts.len() - 3].parse::<u32>(),
            parts[parts.len() - 2].parse::<u32>(),
        );
        if let (Ok(year), Ok(month), Ok(day)) = (y, m, d) {
            if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|date| date.and_hms_opt(0, 0, 0))
            {
                return Timestamp::from_millis(dt.and_utc().timestamp_millis());
            }
        }
    }

    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| Timestamp::from_millis(d.as_millis() as i64))
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn collect_files(dir: &Path, out: &mut Vec<(PathBuf, Timestamp)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if !path.extension().map(|e| e == "part").unwrap_or(false) {
            out.push((path.clone(), captured_at_from_path(&path)));
        }
    }
}

fn prune(kind: &str, dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune(kind, &path);
            // 目录空了就删掉，避免长期运行积累成千上万个空日期目录
            if std::fs::read_dir(&path)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(&path);
            }
        }
    }
    let _ = kind;
}
