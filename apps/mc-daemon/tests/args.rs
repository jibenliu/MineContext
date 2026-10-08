use mc_daemon::{generate_token, parse_args, Args};

#[test]
fn parse_args_defaults_to_system_port() {
    let args = parse_args(&["--data-dir".to_string(), "/tmp/mc".to_string()]).unwrap();

    assert_eq!(args.data_dir, std::path::PathBuf::from("/tmp/mc"));
    assert_eq!(args.port, 0);
    assert_eq!(args.config_path, None);
}

#[test]
fn parse_args_reads_all_options() {
    let args = parse_args(&[
        "--config".to_string(),
        "/tmp/config.toml".to_string(),
        "--data-dir".to_string(),
        "/tmp/mc".to_string(),
        "--port".to_string(),
        "17331".to_string(),
    ])
    .unwrap();

    assert_eq!(
        args.config_path,
        Some(std::path::PathBuf::from("/tmp/config.toml"))
    );
    assert_eq!(args.data_dir, std::path::PathBuf::from("/tmp/mc"));
    assert_eq!(args.port, 17331);
}

#[test]
fn parse_args_rejects_unknown_option() {
    let err = parse_args(&["--nope".to_string()]).unwrap_err();
    assert!(err.contains("未知参数"), "{err}");
}

#[test]
fn parse_args_rejects_bad_port() {
    let err = parse_args(&["--port".to_string(), "not-a-number".to_string()]).unwrap_err();
    assert!(err.contains("--port"), "{err}");
}

#[test]
fn parse_args_requires_value_after_option() {
    let err = parse_args(&["--data-dir".to_string()]).unwrap_err();
    assert!(err.contains("需要一个路径参数"), "{err}");
}

#[test]
fn help_flag_returns_usage() {
    let err = parse_args(&["--help".to_string()]).unwrap_err();
    assert!(err.starts_with("mc-daemon"));
}

#[test]
fn generated_token_is_long_and_unique() {
    let a = generate_token();
    let b = generate_token();

    assert_eq!(a.len(), 64, "两次 UUIDv4 应为 64 个十六进制字符");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b, "token 必须每次不同");
}

#[test]
fn args_helper_sets_data_dir() {
    assert_eq!(
        Args::with_data_dir("/tmp/x").data_dir,
        std::path::PathBuf::from("/tmp/x")
    );
}
