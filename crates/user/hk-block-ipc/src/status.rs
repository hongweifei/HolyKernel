//! 块错误与线协议状态码互转。

use hk_block::BlockError;
use hk_proto::block::{STATUS_BUSY, STATUS_ERROR, STATUS_INVALID, STATUS_RANGE};

/// 线协议状态码转块错误；成功码与未知码按 I/O 错误处理。
pub(crate) fn from_status(status: u8) -> BlockError {
    match status {
        STATUS_RANGE => BlockError::OutOfRange,
        STATUS_INVALID => BlockError::InvalidLength,
        STATUS_BUSY => BlockError::Busy,
        _ => BlockError::Io,
    }
}

/// 块错误转线协议状态码。
pub(crate) fn to_status(error: BlockError) -> u8 {
    match error {
        BlockError::OutOfRange => STATUS_RANGE,
        BlockError::InvalidLength => STATUS_INVALID,
        BlockError::Busy => STATUS_BUSY,
        BlockError::ReadOnly | BlockError::Io => STATUS_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_proto::block::STATUS_OK;

    #[test]
    fn maps_errors_to_status() {
        assert_eq!(to_status(BlockError::OutOfRange), STATUS_RANGE);
        assert_eq!(to_status(BlockError::InvalidLength), STATUS_INVALID);
        assert_eq!(to_status(BlockError::Busy), STATUS_BUSY);
        assert_eq!(to_status(BlockError::ReadOnly), STATUS_ERROR);
        assert_eq!(to_status(BlockError::Io), STATUS_ERROR);
    }

    #[test]
    fn maps_status_to_errors() {
        assert_eq!(from_status(STATUS_RANGE), BlockError::OutOfRange);
        assert_eq!(from_status(STATUS_INVALID), BlockError::InvalidLength);
        assert_eq!(from_status(STATUS_BUSY), BlockError::Busy);
        assert_eq!(from_status(STATUS_ERROR), BlockError::Io);
        assert_eq!(from_status(STATUS_OK), BlockError::Io);
        assert_eq!(from_status(0xFF), BlockError::Io);
    }
}
