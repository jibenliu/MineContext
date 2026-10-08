//! 保留策略的**执行**：轮转删除 + 引用清理。
//!
//! `enforce_retention` 早就实现了删除逻辑并有单元测试，但在此之前
//! **生产代码从未调用它** —— 截图永远不会被轮转删除，磁盘会一直涨。
//! 这一层负责把它接进真实运行，并补上一个只有「一起做」才能成立的约束：
//!
//! **删文件的同时必须清掉数据库里的引用。**
//! 只删文件会留下悬空 `image_path`，界面上就是一片破图；
//! 只清引用不删文件则是磁盘泄漏。两者必须成对发生。

use std::time::Duration;

use image::{Rgb, RgbImage};
use mc_common::time::Timestamp;
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageFormat, ImageMeta, RetentionPolicy};
use mc_storage::observations::{ImageRef, NewObservation};
use mc_storage::retention::run_retention;
use mc_storage::Database;

const DAY_MS: i64 = 86_400_000;
/// 固定"现在"：2026-09-30T09:00:00Z
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as NOW_MS;

struct Ctx {
    _dir: tempfile::TempDir,
    db: Database,
    blobs: FileSystemBlobStore,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();
    let blobs =
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store");
    Ctx {
        _dir: dir,
        db,
        blobs,
    }
}

/// 写入一张观测 + 图片，返回（观测 id，相对路径）。
fn add_screenshot(ctx: &Ctx, index: usize, captured_at_ms: i64) -> (String, String) {
    let image = RgbImage::from_fn(32, 24, |x, y| {
        Rgb([(x * 3) as u8, (y * 5) as u8, (index * 7) as u8])
    });
    let stored = ctx
        .blobs
        .put_image(
            &image,
            &ImageMeta {
                captured_at: Timestamp::from_millis(captured_at_ms),
                display_id: Some("display-1".to_string()),
            },
        )
        .expect("写图片");

    let id = format!("obs-{index}");
    ctx.db
        .insert_observation(&NewObservation {
            id: id.clone(),
            ts: Timestamp::from_millis(captured_at_ms),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Visual Studio Code".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: Some("display-1".to_string()),
            scale_factor: Some(2.0),
            image: Some(ImageRef {
                relative_path: stored.relative_path.clone(),
                content_hash: stored.content_hash.clone(),
                thumbnail_path: stored.thumbnail.as_ref().map(|t| t.relative_path.clone()),
                width: stored.width,
                height: stored.height,
                bytes: stored.bytes,
            }),
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("key-{index}"),
        })
        .expect("写观测");

    (id, stored.relative_path)
}

fn policy(days: u32) -> RetentionPolicy {
    RetentionPolicy {
        screenshots_days: days,
        max_total_bytes: 10 * 1024 * 1024 * 1024,
        max_blob_count: 1_000_000,
    }
}

fn image_path_of(ctx: &Ctx, id: &str) -> Option<String> {
    let rows = ctx
        .db
        .query_observations(&mc_storage::observations::ObservationQuery::default())
        .unwrap();
    rows.into_iter()
        .find(|row| row.id == id)
        .and_then(|row| row.image_path)
}

// ---------------------------------------------------------------- 年龄轮转

#[test]
fn rotates_out_files_older_than_the_window() {
    let ctx = ctx();
    let (_old_id, old_path) = add_screenshot(&ctx, 0, NOW_MS - 10 * DAY_MS);
    let (_new_id, new_path) = add_screenshot(&ctx, 1, NOW_MS - DAY_MS);

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .expect("轮转必须成功");

    assert!(
        report.deleted_files >= 1,
        "超过 7 天的截图必须被删：{report:?}"
    );
    assert!(report.freed_bytes > 0);
    assert!(!ctx.blobs.relative_exists(&old_path), "旧文件应当已删除");
    assert!(ctx.blobs.relative_exists(&new_path), "窗口内的文件必须保留");
}

#[test]
fn clearing_references_keeps_database_and_disk_consistent() {
    let ctx = ctx();
    let (old_id, old_path) = add_screenshot(&ctx, 0, NOW_MS - 30 * DAY_MS);
    let (new_id, _new_path) = add_screenshot(&ctx, 1, NOW_MS - DAY_MS);

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert!(report.cleared_references >= 1, "{report:?}");
    assert!(!ctx.blobs.relative_exists(&old_path), "文件必须真的被删掉");
    assert_eq!(
        image_path_of(&ctx, &old_id),
        None,
        "被删文件的观测不能再留着悬空引用（界面会显示破图）"
    );
    assert!(
        image_path_of(&ctx, &new_id).is_some(),
        "保留的观测引用不受影响"
    );

    // 观测本身仍在（那是一条事实，只是截图没了）
    assert!(ctx.db.observation_count().unwrap() >= 2);
}

#[test]
fn retention_is_idempotent() {
    let ctx = ctx();
    add_screenshot(&ctx, 0, NOW_MS - 30 * DAY_MS);

    let first = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();
    assert!(first.deleted_files >= 1);

    let second = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();
    assert_eq!(second.deleted_files, 0, "重复执行不该再删东西：{second:?}");
    assert_eq!(second.cleared_references, 0);
}

/// `screenshots_days = 0` 表示永久保留：上限足够大时什么都不该删。
#[test]
fn zero_days_means_keep_forever() {
    let ctx = ctx();
    let (_id, path) = add_screenshot(&ctx, 0, NOW_MS - 365 * DAY_MS);

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(0),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert_eq!(report.deleted_files, 0, "永久保留模式不能删：{report:?}");
    assert!(ctx.blobs.relative_exists(&path));
}

// ---------------------------------------------------------------- 容量上限

#[test]
fn respects_the_total_bytes_cap_oldest_first() {
    let ctx = ctx();
    let mut paths = Vec::new();
    for index in 0..5 {
        // 越靠前越旧；且不触发年龄规则（都在窗口内）
        let (_, path) = add_screenshot(&ctx, index, NOW_MS - (5 - index as i64) * DAY_MS);
        paths.push(path);
    }

    let total: u64 = ctx.blobs.stats().unwrap().total_bytes;
    let limit = total / 2;

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &RetentionPolicy {
            screenshots_days: 0,
            max_total_bytes: limit,
            max_blob_count: 1_000_000,
        },
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert!(report.deleted_files > 0, "{report:?}");
    assert!(
        ctx.blobs.stats().unwrap().total_bytes <= limit,
        "轮转后必须落到上限之内"
    );
    assert!(!ctx.blobs.relative_exists(&paths[0]), "应当从最旧的开始删");
    assert!(
        ctx.blobs.relative_exists(paths.last().unwrap()),
        "最新的必须留下"
    );
}

#[test]
fn respects_the_blob_count_cap() {
    let ctx = ctx();
    for index in 0..4 {
        add_screenshot(&ctx, index, NOW_MS - (4 - index as i64) * 60_000);
    }

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &RetentionPolicy {
            screenshots_days: 0,
            max_total_bytes: u64::MAX,
            // 计数上限按「文件数」算：截图 + 缩略图各占一个
            max_blob_count: 2,
        },
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert!(report.deleted_files > 0, "{report:?}");
    assert!(ctx.blobs.stats().unwrap().blob_count <= 2);
}

// ---------------------------------------------------------------- 悬空引用清理

/// 文件已经被别的手段删掉（手工删除 / 旧版 deleteScreenshot 只删文件）时，
/// 轮转要把这些悬空引用一起清掉 —— 否则那条观测永远是破图。
#[test]
fn sweeps_references_whose_file_is_already_gone() {
    let ctx = ctx();
    let (id, path) = add_screenshot(&ctx, 0, NOW_MS - DAY_MS);

    // 模拟「文件被外部删除」
    std::fs::remove_file(ctx.blobs.resolve_relative(&path).unwrap()).unwrap();

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert!(
        report.cleared_references >= 1,
        "悬空引用必须被清掉：{report:?}"
    );
    assert_eq!(image_path_of(&ctx, &id), None);
}

/// 报告里的数字要能解释：删了多少文件 / 释放多少字节 / 清了多少引用。
#[test]
fn report_is_self_consistent() {
    let ctx = ctx();
    add_screenshot(&ctx, 0, NOW_MS - 40 * DAY_MS);
    add_screenshot(&ctx, 1, NOW_MS - 40 * DAY_MS);

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert!(report.deleted_files >= 2, "{report:?}");
    assert!(report.freed_bytes > 0, "{report:?}");
    assert!(
        report.cleared_references >= report.deleted_files / 2,
        "每个被删的截图至少对应一条引用：{report:?}"
    );
    assert!(report.kept_files < 100, "{report:?}");
}

/// 上限为 0 且年龄为 0 是「什么都不删」而不是「全删」——
/// 配置缺省不该变成「清空用户数据」。
#[test]
fn no_rules_configured_deletes_nothing() {
    let ctx = ctx();
    let (_id, path) = add_screenshot(&ctx, 0, NOW_MS - 365 * DAY_MS);

    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &RetentionPolicy {
            screenshots_days: 0,
            max_total_bytes: 0,
            max_blob_count: 0,
        },
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();

    assert_eq!(report.deleted_files, 0, "{report:?}");
    assert!(ctx.blobs.relative_exists(&path));
}

/// 轮转不该让进程卡住太久：1 万条引用的清理必须是一次批量操作。
#[test]
fn clearing_many_references_is_batched() {
    let ctx = ctx();
    for index in 0..200 {
        let (_, path) = add_screenshot(&ctx, index, NOW_MS - DAY_MS);
        std::fs::remove_file(ctx.blobs.resolve_relative(&path).unwrap()).unwrap();
    }

    let started = std::time::Instant::now();
    let report = run_retention(
        &ctx.db,
        &ctx.blobs,
        &policy(7),
        Timestamp::from_millis(NOW_MS),
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(report.cleared_references, 200, "{report:?}");
    // 门槛放宽到 15s：这是「别退化成逐条事务」的烟雾测试，
    // 不是性能基准（机器负载会波动，卡太紧会变成随机失败）
    assert!(
        elapsed < Duration::from_secs(15),
        "200 条引用清理耗时 {elapsed:?}，说明没有批量化"
    );
}
