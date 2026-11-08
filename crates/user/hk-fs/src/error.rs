//! Holy 文件系统错误。

use core::fmt;

use crate::Status;

/// 文件系统操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
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

impl Error {
    /// 映射到服务状态；新增错误变体时只需在这里补一处。
    pub const fn status(self) -> Status {
        match self {
            Self::NotFound => Status::NotFound,
            Self::Exists => Status::Exists,
            Self::Invalid | Self::NotDirectory | Self::IsDirectory => Status::Invalid,
            Self::Io => Status::Io,
            Self::Refused => Status::Permission,
            Self::Stale => Status::Stale,
            Self::Damaged => Status::Damaged,
            Self::ReadOnly => Status::ReadOnly,
            Self::Corrupt
            | Self::NoSpace
            | Self::Unsupported
            | Self::Busy
            | Self::Pinned
            | Self::Quota => Status::Error,
        }
    }

    /// 从服务状态还原错误；未知或成功状态按 I/O 错误处理。
    pub const fn from_status_code(code: u8) -> Self {
        match Status::from_u8(code) {
            Some(Status::NotFound) => Self::NotFound,
            Some(Status::Invalid) => Self::Invalid,
            Some(Status::Io) => Self::Io,
            Some(Status::Permission) => Self::Refused,
            Some(Status::Exists) => Self::Exists,
            Some(Status::Stale) => Self::Stale,
            Some(Status::Damaged) => Self::Damaged,
            Some(Status::ReadOnly) => Self::ReadOnly,
            _ => Self::Io,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "文件系统 I/O 错误",
            Self::Corrupt => "文件系统元数据损坏",
            Self::NoSpace => "文件系统空间不足",
            Self::NotFound => "文件系统对象不存在",
            Self::NotDirectory => "不是集合",
            Self::IsDirectory => "是集合",
            Self::Exists => "名字已占用",
            Self::Invalid => "参数无效",
            Self::Unsupported => "操作不支持",
            Self::ReadOnly => "卷只读",
            Self::Damaged => "卷已损坏",
            Self::Busy => "卷正忙",
            Self::Stale => "期望值已过期",
            Self::Pinned => "对象被快照固定",
            Self::Quota => "超出配额",
            Self::Refused => "句柄无权访问",
        })
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_errors_to_status() {
        assert_eq!(Error::NotFound.status(), Status::NotFound);
        assert_eq!(Error::Exists.status(), Status::Exists);
        assert_eq!(Error::NotDirectory.status(), Status::Invalid);
        assert_eq!(Error::Refused.status(), Status::Permission);
        assert_eq!(Error::Quota.status(), Status::Error);
    }

    #[test]
    fn restores_common_statuses() {
        for code in [1_u8, 2, 4, 5, 6, 7, 8, 9] {
            let error = Error::from_status_code(code);
            assert_eq!(error.status().as_u8(), if code == 2 { 2 } else { code });
        }
        assert_eq!(Error::from_status_code(0), Error::Io);
        assert_eq!(Error::from_status_code(255), Error::Io);
    }
}
