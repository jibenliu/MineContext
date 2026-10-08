//! `mc-cli replay` —— 重建派生表。
//!
//! 这条命令是算法与规则升级的兜底手段：升级后
//! 用户可以自己把历史重算一遍，而不是等系统「下次启动时顺手算」。
//! 因此它必须：能指定投影器、能指定规则文件、输出可核对的数量。

use std::path::{Path, PathBuf};

use mc_cli::{parse_args, run, Command};
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

fn write_events(dir: &Path) -> PathBuf {
    let db_path = dir.join("data").join("minecontext.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let db = mc_storage::Database::open(&db_path).expect("打开数据库");

    let payload = |id: &str, at_ms: i64| {
        serde_json::json!({
            "id": id,
            "at": at_ms,
            "app_name": "VSCode",
            "window_title": "main.rs",
            "domain": null,
            "text": null,
        })
    };

    let base = FIXTURE_EPOCH_MS;
    db.append_events(&[
        mc_storage::NewEvent::new(
            "observation.recorded",
            mc_common::time::Timestamp::from_millis(base),
            payload("obs-1", base),
        ),
        mc_storage::NewEvent::new(
            "observation.recorded",
            mc_common::time::Timestamp::from_millis(base + 60_000),
            payload("obs-2", base + 60_000),
        ),
    ])
    .expect("追加事件");

    db_path
}

// 参数解析
#[test]
fn parse_replay_accepts_projector_rules_and_data_dir() {
    let cmd = parse_args(&[
        "replay".to_string(),
        "--projector".to_string(),
        "activities".to_string(),
        "--data-dir".to_string(),
        "/tmp/mc-test".to_string(),
        "--rules".to_string(),
        "/tmp/rules.yaml".to_string(),
    ])
    .expect("应当能解析");

    match cmd {
        Command::Replay {
            projector,
            data_dir,
            rules,
        } => {
            assert_eq!(projector, "activities");
            assert_eq!(data_dir, PathBuf::from("/tmp/mc-test"));
            assert_eq!(rules, Some(PathBuf::from("/tmp/rules.yaml")));
        }
        other => panic!("解析结果不对：{other:?}"),
    }
}

#[test]
fn parse_replay_defaults_to_activities_projector() {
    let cmd = parse_args(&["replay".to_string()]).expect("应当能解析");
    match cmd {
        Command::Replay { projector, .. } => assert_eq!(projector, "activities"),
        other => panic!("解析结果不对：{other:?}"),
    }
}

// 重放把事件日志变成派生表
#[test]
fn replay_rebuilds_activities_from_the_event_log() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = write_events(dir.path());

    let outcome = run(Command::Replay {
        projector: "activities".to_string(),
        data_dir: dir.path().to_path_buf(),
        rules: None,
    });

    assert_eq!(outcome.exit_code, 0, "重放应当成功：{}", outcome.stdout);
    assert!(
        outcome.stdout.contains("2"),
        "输出要有人能核对的数量（2 条事件）：{}",
        outcome.stdout
    );

    let db = mc_storage::Database::open(&db_path).unwrap();
    let rows = mc_storage::projectors::activities::read_all(&db).expect("读回投影");
    assert_eq!(rows.len(), 1, "两条同应用的观测应当合成一个活动");
    assert_eq!(rows[0].evidence, vec!["obs-1", "obs-2"]);
}

// 规则文件生效：命中规则的活动带 rule 来源
#[test]
fn replay_honours_the_rules_file() {
    let dir = tempfile::tempdir().unwrap();
    write_events(dir.path());
    let rules_path = dir.path().join("activities.yaml");
    std::fs::write(
        &rules_path,
        r#"
version: 1
activities:
  - id: coding
    name: 写代码
    category: 开发
    triggers:
      apps: [VSCode]
"#,
    )
    .unwrap();

    let outcome = run(Command::Replay {
        projector: "activities".to_string(),
        data_dir: dir.path().to_path_buf(),
        rules: Some(rules_path),
    });

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);

    let db = mc_storage::Database::open(dir.path().join("data/minecontext.db")).unwrap();
    let rows = mc_storage::projectors::activities::read_all(&db).unwrap();
    assert_eq!(rows[0].title, "写代码");
    assert_eq!(
        rows[0].origin,
        mc_domain::activity::Provenance::Rule {
            rule_id: "coding".to_string()
        }
    );
}

#[test]
fn replay_reports_a_broken_rules_file_instead_of_silently_ignoring_it() {
    let dir = tempfile::tempdir().unwrap();
    write_events(dir.path());
    let rules_path = dir.path().join("activities.yaml");
    std::fs::write(&rules_path, "version: 99\nactivities: []\n").unwrap();

    let outcome = run(Command::Replay {
        projector: "activities".to_string(),
        data_dir: dir.path().to_path_buf(),
        rules: Some(rules_path),
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stdout.contains("规则"),
        "规则文件有问题必须说清楚，不能装作没看见：{}",
        outcome.stdout
    );
}

#[test]
fn replay_rejects_unknown_projectors() {
    let dir = tempfile::tempdir().unwrap();
    write_events(dir.path());

    let outcome = run(Command::Replay {
        projector: "stages".to_string(),
        data_dir: dir.path().to_path_buf(),
        rules: None,
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stdout.contains("activities"),
        "要告诉用户当前支持哪些投影器：{}",
        outcome.stdout
    );
}
