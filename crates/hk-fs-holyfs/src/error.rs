//! HolyFS 操作错误。

use core::fmt;

/// HolyFS 操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolyError {
    /// 块设备 I/O 失败。
    Io,
    /// 磁盘结构、校验和或记录无效。
    Corrupt,
    /// 卷空间不足。
    NoSpace,
    /// 文档、修订或名字不存在。
    NotFound,
    /// 目标不是集合。
    NotDirectory,
    /// 目标是集合，不能按内容读取。
    IsDirectory,
    /// 名字已被占用。
    Exists,
    /// 参数无效。
    Invalid,
    /// 当前格式或操作不受支持。
    Unsupported,
    /// 卷只读或操作需要写权限。
    ReadOnly,
    /// 卷已损坏，需要修复。
    Damaged,
    /// 卷正忙，无法切换或提交。
    Busy,
    /// 期望值与当前 head/generation 不一致。
    Stale,
    /// 对象被快照固定。
    Pinned,
    /// 超出卷配额。
    Quota,
    /// 句柄权利不足或令牌过期。
    Refused,
}

impl fmt::Display for HolyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "HolyFS I/O 错误",
            Self::Corrupt => "HolyFS 元数据损坏",
            Self::NoSpace => "HolyFS 空间不足",
            Self::NotFound => "HolyFS 对象不存在",
            Self::NotDirectory => "HolyFS 不是集合",
            Self::IsDirectory => "HolyFS 是集合",
            Self::Exists => "HolyFS 名字已占用",
            Self::Invalid => "HolyFS 参数无效",
            Self::Unsupported => "HolyFS 操作不支持",
            Self::ReadOnly => "HolyFS 卷只读",
            Self::Damaged => "HolyFS 卷已损坏",
            Self::Busy => "HolyFS 卷正忙",
            Self::Stale => "HolyFS 期望值已过期",
            Self::Pinned => "HolyFS 对象被快照固定",
            Self::Quota => "HolyFS 超出配额",
            Self::Refused => "HolyFS 句柄无权访问",
        })
    }
}

impl core::error::Error for HolyError {}

pub(crate) fn map_io(_error: hk_block::BlockError) -> HolyError {
    HolyError::Io
}
