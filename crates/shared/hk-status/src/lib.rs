//! 服务状态枚举：各层共用的状态词表。
//!
//! 线协议、文件系统错误与 VFS 错误都映射到同一组状态值；数值只在
//! [`Status`] 定义一次，协议常量与领域 `status()` 都从它派生。

#![no_std]

/// 服务状态。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// 成功。
    Ok = 0,
    /// 未找到。
    NotFound = 1,
    /// 参数非法。
    Invalid = 2,
    /// 后端错误。
    Error = 3,
    /// 服务不可用。
    Io = 4,
    /// 权限不足。
    Permission = 5,
    /// 已存在。
    Exists = 6,
    /// 期望值或令牌代际已过期。
    Stale = 7,
    /// 卷已损坏并进入只读。
    Damaged = 8,
    /// 卷当前只读。
    ReadOnly = 9,
    /// 访问范围越界。
    Range = 10,
    /// 队列已满。
    Busy = 11,
}

impl Status {
    /// 返回协议状态值。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// 从协议状态值解码；未知值返回 `None`。
    pub const fn from_u8(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Ok,
            1 => Self::NotFound,
            2 => Self::Invalid,
            3 => Self::Error,
            4 => Self::Io,
            5 => Self::Permission,
            6 => Self::Exists,
            7 => Self::Stale,
            8 => Self::Damaged,
            9 => Self::ReadOnly,
            10 => Self::Range,
            11 => Self::Busy,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Status] = &[
        Status::Ok,
        Status::NotFound,
        Status::Invalid,
        Status::Error,
        Status::Io,
        Status::Permission,
        Status::Exists,
        Status::Stale,
        Status::Damaged,
        Status::ReadOnly,
        Status::Range,
        Status::Busy,
    ];

    #[test]
    fn round_trips_every_status() {
        for status in ALL {
            assert_eq!(Status::from_u8(status.as_u8()), Some(*status));
        }
        assert_eq!(Status::from_u8(12), None);
    }
}
