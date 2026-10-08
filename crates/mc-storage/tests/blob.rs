//! 截图 blob 存储与保留策略。
//!
//! 对应 ：数据库只存路径与哈希，图片落文件系统。
//! 教训是「磁盘与 DB 一起失控」，所以保留策略必须和写入一起做。

use image::{Rgb, RgbImage};
use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageFormat, ImageMeta, RetentionPolicy};

fn sample_image(width: u32, height: u32, seed: u8) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        Rgb([
            ((x + seed as u32) % 256) as u8,
            ((y + seed as u32) % 256) as u8,
            128,
        ])
    })
}

fn store(dir: &std::path::Path) -> FileSystemBlobStore {
    FileSystemBlobStore::new(dir.to_path_buf(), ImageFormat::Png).expect("store 必须可构造")
}

/// 2026-09-30T09:00:00Z —— 路径断言依赖它
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as NOW_MS;

fn meta(at_ms: i64) -> ImageMeta {
    ImageMeta {
        captured_at: Timestamp::from_millis(at_ms),
        display_id: Some("display-1".to_string()),
    }
}

// ---------------------------------------------------------------- 1.26 编码与缩略图

#[test]
fn image_is_encoded_and_thumbnail_created() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let stored = store
        .put_image(&sample_image(640, 400, 1), &meta(1_756_000_000_000))
        .unwrap();

    assert!(stored.path.exists(), "主图必须落盘");
    assert!(stored.bytes > 0);
    assert!(
        !stored.content_hash.is_empty(),
        "必须记录内容哈希（幂等键与去重用）"
    );

    let thumbnail = stored.thumbnail.as_ref().expect("必须生成缩略图");
    assert!(thumbnail.path.exists());
    assert!(
        thumbnail.width <= 320,
        "缩略图不应超过 320px 宽，实际 {}",
        thumbnail.width
    );
    assert!(thumbnail.bytes > 0);
}

#[test]
fn blob_path_follows_date_layout() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    // 2026-09-30T09:00:00Z；用 > 320 宽的图以确保同时生成缩略图
    let stored = store
        .put_image(&sample_image(640, 400, 2), &meta(NOW_MS))
        .unwrap();

    let relative = stored.relative_path.replace('\\', "/");
    assert!(
        relative.starts_with("screenshots/2026/09/30/"),
        "路径应按日期分目录，实际 {relative}"
    );
    assert!(relative.ends_with(".png"));
    let thumb = &stored.thumbnail.as_ref().unwrap().relative_path;
    assert!(
        thumb
            .replace('\\', "/")
            .starts_with("thumbnails/2026/09/30/"),
        "缩略图应单独分目录，实际 {thumb}"
    );
}

#[test]
fn identical_content_is_stored_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    // 宽度 > 320 才会生成缩略图（小图缩放没有意义）
    let image = sample_image(640, 400, 3);

    let first = store.put_image(&image, &meta(NOW_MS)).unwrap();
    let second = store.put_image(&image, &meta(NOW_MS)).unwrap();

    // 内容寻址：同一份内容不应在磁盘上留两份
    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.path, second.path);

    let stats = store.stats().unwrap();
    assert_eq!(stats.blob_count, 2, "主图 + 缩略图");
}

// ---------------------------------------------------------------- 1.25 原子写入

#[test]
fn blob_write_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    store
        .put_image(&sample_image(200, 120, 4), &meta(NOW_MS))
        .unwrap();

    // 目录里不应残留任何临时文件 —— 崩溃或中断不能留下半张图
    let mut leftovers = Vec::new();
    for entry in walk(dir.path()) {
        let name = entry.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with('.') || name.ends_with(".tmp") || name.ends_with(".part") {
            leftovers.push(name);
        }
    }
    assert!(leftovers.is_empty(), "残留了临时文件: {leftovers:?}");
}

/// 目录中不应存在「0 字节的图片」——那正是写了一半的痕迹。
#[test]
fn no_zero_byte_images_after_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    for seed in 0..5u8 {
        store
            .put_image(&sample_image(120, 80, seed), &meta(NOW_MS))
            .unwrap();
    }

    for entry in walk(dir.path()) {
        if entry.extension().map(|e| e == "png").unwrap_or(false) {
            let size = std::fs::metadata(&entry).unwrap().len();
            assert!(
                size > 0,
                "存在 0 字节图片（写入未完成）: {}",
                entry.display()
            );
        }
    }
}

// ---------------------------------------------------------------- 1.30 文件丢失

#[test]
fn missing_blob_file_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let stored = store
        .put_image(&sample_image(640, 400, 5), &meta(NOW_MS))
        .unwrap();

    // 用户在 Finder 里删掉了文件
    std::fs::remove_file(&stored.path).unwrap();

    let error = store.get(&stored.blob_id).unwrap_err();
    assert!(
        matches!(
            error.code(),
            ErrorCode::StorageBlobMissing
                | ErrorCode::StorageUnavailable
                | ErrorCode::StorageCorrupt
        ),
        "缺失文件应返回「文件不存在」类错误，实际 {:?}",
        error.code()
    );

    // 但统计与列举必须仍可用（不能因为一张图丢了就整个存储不可用）
    let stats = store.stats().unwrap();
    assert!(stats.blob_count >= 1);
}

#[test]
fn get_returns_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let stored = store
        .put_image(&sample_image(64, 40, 6), &meta(NOW_MS))
        .unwrap();

    let bytes = store.get(&stored.blob_id).unwrap();
    assert_eq!(bytes.len() as u64, stored.bytes, "读回的字节数应与记录一致");
}

// ---------------------------------------------------------------- 1.27 保留策略

#[test]
fn retention_deletes_old_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let day = 86_400_000i64;
    let now = 1_790_000_400_000i64;

    let old = store
        .put_image(&sample_image(640, 400, 10), &meta(now - 30 * day))
        .unwrap();
    let recent = store
        .put_image(&sample_image(640, 400, 11), &meta(now - day))
        .unwrap();

    let policy = RetentionPolicy {
        screenshots_days: 7,
        max_total_bytes: u64::MAX,
        ..Default::default()
    };
    let report = store
        .enforce_retention(Timestamp::from_millis(now), &policy)
        .unwrap();

    assert!(
        report.deleted >= 2,
        "主图与缩略图都应被删除，实际 {}",
        report.deleted
    );
    assert!(report.freed_bytes > 0);
    assert!(!old.path.exists(), "30 天前的 blob 必须被清理");
    assert!(recent.path.exists(), "昨天的 blob 必须保留");
    assert!(recent.thumbnail.as_ref().unwrap().path.exists());
}

#[test]
fn retention_keeps_everything_when_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let day = 86_400_000i64;
    let now = 1_790_000_400_000i64;
    let old = store
        .put_image(&sample_image(64, 40, 12), &meta(now - 365 * day))
        .unwrap();

    let policy = RetentionPolicy {
        screenshots_days: 0, // 0 = 永久保留
        max_total_bytes: u64::MAX,
        ..Default::default()
    };
    let report = store
        .enforce_retention(Timestamp::from_millis(now), &policy)
        .unwrap();

    assert_eq!(report.deleted, 0, "关闭保留策略时不应删除任何东西");
    assert!(old.path.exists());
}

#[test]
fn retention_respects_max_total_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let day = 86_400_000i64;
    let now = 1_790_000_400_000i64;

    // 造 20 张互不相同、体积可观的图
    for seed in 0..20u8 {
        store
            .put_image(
                &sample_image(300, 200, seed),
                &meta(now - (seed as i64) * day),
            )
            .unwrap();
    }
    let before = store.stats().unwrap().total_bytes;
    assert!(before > 0);

    let policy = RetentionPolicy {
        screenshots_days: 365,
        max_total_bytes: before / 2,
        ..Default::default()
    };
    let report = store
        .enforce_retention(Timestamp::from_millis(now), &policy)
        .unwrap();

    let after = store.stats().unwrap().total_bytes;
    assert!(report.deleted > 0, "超出字节上限时必须删除");
    assert!(
        after <= before / 2,
        "清理后应落回上限内：before={before} after={after}"
    );
}

#[test]
fn retention_respects_max_blob_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let now = 1_790_000_400_000i64;
    for seed in 0..10u8 {
        store
            .put_image(
                &sample_image(64, 40, seed),
                &meta(now - seed as i64 * 1_000),
            )
            .unwrap();
    }

    let policy = RetentionPolicy {
        screenshots_days: 365,
        max_total_bytes: u64::MAX,
        max_blob_count: 6,
    };
    store
        .enforce_retention(Timestamp::from_millis(now), &policy)
        .unwrap();

    let stats = store.stats().unwrap();
    assert!(
        stats.blob_count <= 6,
        "清理后条目数必须落回上限内，实际 {}",
        stats.blob_count
    );
}

#[test]
fn retention_removes_empty_date_directories() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let day = 86_400_000i64;
    let now = 1_790_000_400_000i64;
    store
        .put_image(&sample_image(64, 40, 20), &meta(now - 100 * day))
        .unwrap();

    let policy = RetentionPolicy {
        screenshots_days: 7,
        max_total_bytes: u64::MAX,
        ..Default::default()
    };
    store
        .enforce_retention(Timestamp::from_millis(now), &policy)
        .unwrap();

    // 清理后不应留下空的日期目录（否则长期运行会积累成千上万个空目录）
    let remaining_files = walk(dir.path()).count();
    assert_eq!(remaining_files, 0, "应当只剩空目录结构");
}

// ---------------------------------------------------------------- 1.29 磁盘写入失败

#[cfg(unix)]
#[test]
fn write_failure_is_a_typed_error_not_a_panic() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let readonly = dir.path().join("readonly");
    std::fs::create_dir_all(&readonly).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();

    let store = store(&readonly);
    let result = store.put_image(&sample_image(64, 40, 30), &meta(NOW_MS));

    // 恢复权限，便于 tempdir 清理
    let _ = std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700));

    let error = result.expect_err("只读目录下写入必须失败");
    assert!(
        matches!(
            error.code(),
            ErrorCode::StorageDiskFull | ErrorCode::StorageUnavailable
        ),
        "应是可重试的存储错误，实际 {:?}",
        error.code()
    );
}

// ---------------------------------------------------------------- 工具

fn walk(root: &std::path::Path) -> impl Iterator<Item = std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.into_iter()
}

// ---------------------------------------------------------------- 路径安全

/// 路径校验是安全属性，必须在 blob store 内部（唯一实现处）完成。
#[test]
fn resolve_relative_rejects_unsafe_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    for evil in [
        "/etc/passwd",
        "../../../etc/passwd",
        "screenshots/../../etc/passwd",
        "thumbnails/../../secret",
        "C:/windows/system32",
        "",
        "other/place.png",
    ] {
        let error = store
            .resolve_relative(evil)
            .expect_err(&format!("`{evil}` 必须被拒绝"));
        assert_eq!(
            error.code(),
            ErrorCode::StorageInvalidBlobPath,
            "`{evil}` 应返回「引用无效」而不是别的错误"
        );
        assert!(
            !error.user_message().contains("数据库"),
            "用户可见文案不应误导成数据库问题：{}",
            error.user_message()
        );
    }
}

#[test]
fn resolve_relative_accepts_controlled_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    for good in [
        "screenshots/2026/09/30/abc.png",
        "thumbnails/2026/09/30/abc-thumb.png",
    ] {
        assert!(store.resolve_relative(good).is_ok(), "`{good}` 应当被接受");
    }
}

#[test]
fn missing_blob_reports_a_specific_code() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let error = store
        .read_relative("screenshots/2026/09/30/nope.png")
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::StorageBlobMissing);
    assert!(error.remediation().is_some(), "应提示保留策略相关建议");
}
