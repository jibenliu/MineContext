//! 配置写回：保存设置必须「要么完整生效，要么完全不动」。
//!
//! 这是「UI 改设置 → 用户配置层 → 热重载」的底层。三条不变量：
//! 先校验后落盘、合并不是覆盖、原子替换。每一条都对应一类真实事故，
//! 因此每条都有独立测试。

use mc_config::load::{load, ConfigHandle, LayerSource, LoadRequest};
use mc_config::write::{apply_patch, read_user_config, write_atomic};

fn request_for(path: &std::path::Path) -> LoadRequest {
    LoadRequest {
        layers: vec![LayerSource::File(path.to_path_buf())],
        env: Vec::new(),
        read_process_env: false,
    }
}

/// 首次写入前用户文件还不存在 —— 这时启动请求里没有文件层
/// （显式传入不存在的 `--config` 仍然报错，那是防拼错，与此无关）。
fn defaults_only() -> LoadRequest {
    LoadRequest::default()
}

fn patch(toml_text: &str) -> toml::Value {
    toml::from_str(toml_text).expect("补丁必须是合法 TOML")
}

// ---------------------------------------------------------------- 基本写回

#[test]
fn writes_the_patch_and_reloads_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let request = request_for(&path);

    let handle = ConfigHandle::new(load(&defaults_only()).expect("首次加载"));
    assert_eq!(handle.current().config.capture.interval_secs, 15, "默认值");

    let loaded = apply_patch(
        &request,
        &path,
        &handle,
        &patch("[capture]\ninterval_secs = 5\n"),
    )
    .expect("写回必须成功");

    assert_eq!(loaded.config.capture.interval_secs, 5);
    // 句柄也热更新了（不用重启）
    assert_eq!(handle.current().config.capture.interval_secs, 5);

    // 磁盘上是真的写了
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("interval_secs = 5"), "{text}");
}

#[test]
fn creates_the_user_config_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let request = request_for(&path);
    let handle = ConfigHandle::new(load(&defaults_only()).unwrap());

    apply_patch(
        &request,
        &path,
        &handle,
        &patch("[capture]\ninterval_secs = 7\n"),
    )
    .unwrap();
    assert!(path.exists(), "缺失时应当创建（含父目录）");
}

// ---------------------------------------------------------------- 合并不是覆盖

#[test]
fn patch_deep_merges_instead_of_replacing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let request = request_for(&path);

    std::fs::write(
        &path,
        "[capture]\ninterval_secs = 30\ntarget_ids = [\"display-1\"]\n[general]\ntimezone = \"Asia/Shanghai\"\n",
    )
    .unwrap();
    let handle = ConfigHandle::new(load(&request).unwrap());

    // 只改时段，不该碰 target_ids 与时区
    apply_patch(
        &request,
        &path,
        &handle,
        &patch("[capture]\nretention_days = 3\n"),
    )
    .unwrap();

    let config = handle.current();
    assert_eq!(config.config.capture.retention_days, 3);
    assert_eq!(config.config.capture.interval_secs, 30, "同表其它字段保留");
    assert_eq!(
        config.config.capture.target_ids,
        vec!["display-1".to_string()],
        "数组字段保留"
    );
    assert_eq!(
        config.config.general.timezone.as_deref(),
        Some("Asia/Shanghai"),
        "其它表保留"
    );
}

// ---------------------------------------------------------------- 先校验后落盘

/// 校验失败的补丁**不能**碰磁盘（否则一次手滑就让 daemon 起不来）
#[test]
fn invalid_patch_is_rejected_without_touching_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let request = request_for(&path);

    let original = "[capture]\ninterval_secs = 30\n";
    std::fs::write(&path, original).unwrap();
    let handle = ConfigHandle::new(load(&request).unwrap());

    // 未知字段（`deny_unknown_fields`）→ 必须被拒绝
    let error = apply_patch(
        &request,
        &path,
        &handle,
        &patch("[capture]\nnot_a_field = 1\n"),
    )
    .expect_err("未知字段必须被拒绝");

    assert_eq!(error.code(), mc_common::error::ErrorCode::ConfigInvalid);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "被拒绝的补丁不能改动文件"
    );
    assert_eq!(
        handle.current().config.capture.interval_secs,
        30,
        "内存也不变"
    );
}

/// 用户手写的文件解析不了时：拒绝写入，而不是覆盖掉他的内容
#[test]
fn malformed_existing_file_is_not_clobbered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let broken = "[capture\ninterval_secs = 30\n";
    std::fs::write(&path, broken).unwrap();

    // 加载会先失败，因此这里直接验证读取时报错
    let error = read_user_config(&path).expect_err("解析不了必须报错");
    assert_eq!(error.code(), mc_common::error::ErrorCode::ConfigInvalid);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

// ---------------------------------------------------------------- 原子替换

#[test]
fn atomic_write_leaves_no_temp_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    write_atomic(&path, "[capture]\ninterval_secs = 9\n").unwrap();
    write_atomic(&path, "[capture]\ninterval_secs = 10\n").unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[capture]\ninterval_secs = 10\n"
    );

    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "不该留下临时文件：{leftovers:?}");
}

/// 空文件等同于「没有用户配置」，不该被当成解析失败
#[test]
fn empty_file_is_treated_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "   \n").unwrap();

    let value = read_user_config(&path).unwrap();
    assert!(value.as_table().unwrap().is_empty());
}

// ---------------------------------------------------------------- 环境层参与校验

/// 校验用的分层必须与启动一致：环境变量层也要参与，
/// 否则「保存时通过、重启后失败」这种最难受的组合就会出现。
#[test]
fn validation_uses_the_same_layers_as_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let request = LoadRequest {
        layers: vec![LayerSource::File(path.clone())],
        env: vec![(
            "MC_CONFIG__general__timezone".to_string(),
            "\"UTC\"".to_string(),
        )],
        read_process_env: false,
    };
    // 文件层要求文件存在（防拼错）；这里先落一个空文件
    std::fs::write(&path, "").unwrap();
    let handle = ConfigHandle::new(load(&request).unwrap());

    apply_patch(
        &request,
        &path,
        &handle,
        &patch("[capture]\ninterval_secs = 11\n"),
    )
    .unwrap();

    // 环境层优先级更高，因此时区仍然是 UTC
    assert_eq!(
        handle.current().config.general.timezone.as_deref(),
        Some("UTC")
    );
    assert_eq!(handle.current().config.capture.interval_secs, 11);
}
