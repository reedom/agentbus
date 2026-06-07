//! Store layout (spec section 5): a single 0700 directory holding bus.db
//! and per-instance inbox spool files.

use std::path::{Path, PathBuf};

/// Store root: $AGENTBUS_DIR when set, else ~/.agentbus.
pub fn base_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("AGENTBUS_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".agentbus")
}

pub fn inbox_dir(base: &Path) -> PathBuf {
    base.join("inbox")
}

pub fn db_path(base: &Path) -> PathBuf {
    base.join("bus.db")
}

/// Create base and inbox dirs, tightening permissions to 0700 even when
/// they already exist (spec section 9).
pub fn ensure_layout(base: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for dir in [base.to_path_buf(), inbox_dir(base)] {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn ensure_layout_creates_0700_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("bus");
        ensure_layout(&base).unwrap();
        for dir in [&base, &inbox_dir(&base)] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{dir:?}");
        }
        // Idempotent on a second call.
        ensure_layout(&base).unwrap();
    }
}
