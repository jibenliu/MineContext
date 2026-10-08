//! 文件名必须被关在自己的目录里。
//!
//! `GET /api/files/{name}/data` 与 `POST /api/files` 都直接来自渲染层。
//! 旧 Electron 实现是 `path.join(filesDir, fileName)` —— 传入
//! `../../../../.ssh/id_rsa` 就会**读出任意文件**（旧版在本地进程里没人管）。
//! HTTP 面必须自己把关：token 只证明「是本应用」，不证明「参数可信」。
//!
//! 校验是纯函数（`mc-common` 里没有 IO），因此可以穷举边界：
//! 绝对路径、`..` 穿越、目录分隔符、空名字、`.` / `..` 本身。

use mc_common::error::ErrorCode;
use mc_common::fs::resolve_within;

#[test]
fn accepts_plain_file_names() {
    let dir = std::path::Path::new("/tmp/uploads");
    assert_eq!(
        resolve_within(dir, "report.pdf").unwrap(),
        std::path::PathBuf::from("/tmp/uploads/report.pdf")
    );
    // 空格、中文、多个点都是合法文件名
    assert_eq!(
        resolve_within(dir, "2026-09-30 日报.final.md").unwrap(),
        std::path::PathBuf::from("/tmp/uploads/2026-09-30 日报.final.md")
    );
    // 以点开头的隐藏文件是合法的（前端自己决定要不要展示）
    assert!(resolve_within(dir, ".gitignore").is_ok());
}

#[test]
fn rejects_parent_traversal() {
    let dir = std::path::Path::new("/tmp/uploads");
    for name in [
        "../secret.txt",
        "../../../../.ssh/id_rsa",
        "a/../../b.txt",
        "..",
        ".",
        "sub/../../..",
    ] {
        let error = resolve_within(dir, name).expect_err(&format!("{name:?} 必须被拒绝"));
        assert_eq!(error.code(), ErrorCode::StorageInvalidBlobPath, "{name:?}");
    }
}

#[test]
fn rejects_absolute_paths() {
    let dir = std::path::Path::new("/tmp/uploads");
    for name in ["/etc/passwd", "/tmp/uploads/ok.txt"] {
        assert!(
            resolve_within(dir, name).is_err(),
            "{name:?} 是绝对路径，必须被拒绝"
        );
    }
}

#[test]
fn rejects_directory_separators() {
    let dir = std::path::Path::new("/tmp/uploads");
    for name in ["sub/file.txt", "sub\\file.txt"] {
        assert!(
            resolve_within(dir, name).is_err(),
            "{name:?} 含目录分隔符，必须被拒绝"
        );
    }
}

#[test]
fn rejects_empty_and_whitespace_names() {
    let dir = std::path::Path::new("/tmp/uploads");
    for name in ["", "   ", "\t"] {
        assert!(resolve_within(dir, name).is_err(), "{name:?} 必须被拒绝");
    }
}

/// 拒绝时的报错要说清「为什么」，而不是只说「无效路径」。
#[test]
fn error_explains_the_reason() {
    let dir = std::path::Path::new("/tmp/uploads");
    let error = resolve_within(dir, "../x").unwrap_err();
    assert!(
        error.detail().contains("..") || error.detail().contains("穿越"),
        "报错要指出问题所在：{}",
        error.detail()
    );
}

/// 目录本身也要挡住：`/api/files/uploads/data` 不能把整目录当文件读。
#[test]
fn resolves_inside_a_relative_directory_too() {
    let dir = std::path::Path::new("uploads");
    assert_eq!(
        resolve_within(dir, "a.txt").unwrap(),
        std::path::PathBuf::from("uploads/a.txt")
    );
}

// W2：目录里放一个指向外面的软链时，join 之后读写会跟出去
#[test]
fn resolve_within_rejects_symlink_escaping_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        // 名字本身合法（单层、无 ..、无分隔符），但它指向目录外，必须在这一步被拒：
        // 放行的话调用方拿到这个路径再拼一层就读到了外面的文件。
        let escaped = mc_common::fs::resolve_within(dir.path(), "link");
        assert!(escaped.is_err(), "指向目录外的软链必须被拒绝：{escaped:?}");
    }

    // 正常名字仍然通过
    assert!(mc_common::fs::resolve_within(dir.path(), "note.txt").is_ok());
}
