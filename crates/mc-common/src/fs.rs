//! 文件系统安全工具。
//!
//! 集中一处的原因：`0600` 这类安全属性最容易被复制粘贴时漏掉。
//! 统一入口保证「创建即受限」，而不是先写后 `chmod`
//! —— 后者在一个短暂窗口内文件是 0644，密钥可能被读到。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{AppError, ErrorCode};

/// 写入一个仅属主可读的文件（Unix 下 0600）。
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    #[cfg(not(unix))]
    {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }
}

/// 字符串版本，便于直接写 JSON / TOML。
pub fn write_private_str(path: &Path, content: &str) -> std::io::Result<()> {
    write_private(path, content.as_bytes())
}

/// 把一个**不可信的名字**解析成 `dir` 内的路径。
///
/// 直接 `dir.join(name)` 是不安全的：`../../.ssh/id_rsa` 会被原样拼上，
/// 调用方（或一次误配置）就能读到目录外的文件。这里只接受
/// 「单层、不含分隔符、不含 `..`」的名字。
///
/// 拒绝的理由会写进错误详情 —— 「无效路径」这种提示对排查毫无帮助。
pub fn resolve_within(dir: &Path, name: &str) -> Result<PathBuf, AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid("文件名为空"));
    }

    let as_path = Path::new(trimmed);
    if as_path.is_absolute() {
        return Err(invalid("不接受绝对路径"));
    }
    // `..` 先判：`../x` 的主要问题是穿越，不是「含分隔符」——
    // 报错要指出真正的原因，否则排查时会被引向错误的方向。
    if trimmed == "." || trimmed == ".." || trimmed.contains("..") {
        return Err(invalid("文件名不能包含 `..`（目录穿越）"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(invalid("文件名不能包含目录分隔符"));
    }

    let candidate = dir.join(trimmed);

    // 符号链接逃逸：名字本身合法，但目录里可能有一个指向**外面**的软链。
    // 只校验「父目录解析之后仍在 dir 内」——目标文件可能还不存在（上传场景），
    // 不能对候选路径本身做 canonicalize。
    let base = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    // 候选**自身**存在时先看它解析到哪儿：
    // `dir/link`（最后一段就是指向外面的软链）必须在这一步被拦下，
    // 否则调用方拿到这个路径后继续拼 `link/secret` 就走出去了。
    if let Ok(resolved) = candidate.canonicalize() {
        if !resolved.starts_with(&base) {
            return Err(invalid("文件名解析后落在数据目录之外（符号链接）"));
        }
    }

    let parent = candidate.parent().unwrap_or(&base);
    match parent.canonicalize() {
        Ok(resolved) if !resolved.starts_with(&base) => {
            Err(invalid("文件名的父目录解析后落在数据目录之外（符号链接）"))
        }
        // 父目录还不存在：形状校验通过，交给调用方去建（这一段 TOCTOU 由调用方承担）
        _ => Ok(candidate),
    }
}

fn invalid(reason: &str) -> AppError {
    AppError::new(
        ErrorCode::StorageInvalidBlobPath,
        format!("{reason}；只接受所在目录内的单层文件名"),
    )
}
