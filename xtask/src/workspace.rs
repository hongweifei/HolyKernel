//! 仓库路径。

use std::path::{Path, PathBuf};

/// 返回仓库根目录。
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 位于仓库子目录")
        .to_path_buf()
}
