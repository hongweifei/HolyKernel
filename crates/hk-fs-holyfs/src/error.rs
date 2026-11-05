//! HolyFS 操作错误；由 `hk-fs` 契约定义。

pub use hk_fs::Error as HolyError;

pub(crate) fn map_io(_error: hk_block::BlockError) -> HolyError {
    HolyError::Io
}
