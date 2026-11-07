//! HMP 对象层错误。

use core::fmt;

/// HMP 对象层错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HmpError {
    /// 长度为零或参数非法。
    InvalidSize,
    /// 地址或长度未按页对齐。
    Unaligned,
    /// 区间超出能力范围。
    OutOfRange,
    /// 同时请求可写与可执行。
    WxConflict,
    /// 只读能力收到写操作。
    ReadOnly,
    /// 操作与能力类型不匹配。
    WrongCapability,
    /// 映射已存在。
    AlreadyMapped,
    /// 映射不存在。
    NotMapped,
    /// Capacity 容量不足。
    OutOfCapacity,
    /// 能力缺少 Provider。
    ProviderMissing,
    /// 权限不足。
    PermissionDenied,
    /// Ledger 配额不足。
    LedgerLimit,
}

impl fmt::Display for HmpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => formatter.write_str("长度或参数非法"),
            Self::Unaligned => formatter.write_str("地址或长度未按页对齐"),
            Self::OutOfRange => formatter.write_str("区间超出能力范围"),
            Self::WxConflict => formatter.write_str("映射同时要求可写与可执行"),
            Self::ReadOnly => formatter.write_str("只读能力拒绝写操作"),
            Self::WrongCapability => formatter.write_str("操作与能力类型不匹配"),
            Self::AlreadyMapped => formatter.write_str("映射已存在"),
            Self::NotMapped => formatter.write_str("映射不存在"),
            Self::OutOfCapacity => formatter.write_str("Capacity 容量不足"),
            Self::ProviderMissing => formatter.write_str("能力缺少 Provider"),
            Self::PermissionDenied => formatter.write_str("权限不足"),
            Self::LedgerLimit => formatter.write_str("Ledger 配额不足"),
        }
    }
}

impl core::error::Error for HmpError {}
