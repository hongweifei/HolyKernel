//! HolyFS 稳定身份类型。

use core::fmt;

/// 文档身份；128 位，卷内稳定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(u128);

impl DocId {
    /// 根文档身份。
    pub const ROOT: Self = Self(1);

    /// 构造文档身份。
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// 返回原始值。
    pub const fn get(self) -> u128 {
        self.0
    }

    /// 编码为小端字节。
    pub const fn to_le_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    /// 从小端字节解码。
    pub const fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_le_bytes(bytes))
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:032x}", self.0)
    }
}

/// 修订身份；卷内单调编号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevisionId(u64);

impl RevisionId {
    /// 根集合修订。
    pub const ROOT: Self = Self(1);

    /// 构造修订身份。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回原始值。
    pub const fn get(self) -> u64 {
        self.0
    }

    /// 编码为小端字节。
    pub const fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    /// 从小端字节解码。
    pub const fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// 卷身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeId(u128);

impl VolumeId {
    /// 构造卷身份。
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// 返回原始值。
    pub const fn get(self) -> u128 {
        self.0
    }

    /// 编码为小端字节。
    pub const fn to_le_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    /// 从小端字节解码。
    pub const fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_le_bytes(bytes))
    }
}

/// 句柄访问权利。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleRights(u32);

impl HandleRights {
    /// 查询类型与元数据。
    pub const INSPECT: Self = Self(1 << 0);
    /// 读取内容。
    pub const READ: Self = Self(1 << 1);
    /// 写入、提交与集合变更。
    pub const WRITE: Self = Self(1 << 2);
    /// 绑定、解绑、改名与标签。
    pub const BIND: Self = Self(1 << 3);
    /// 快照、恢复、封存、回收。
    pub const ADMIN: Self = Self(1 << 4);
    /// 通过 IPC 转移句柄。
    pub const TRANSFER: Self = Self(1 << 5);

    /// 空权利。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 构造权利集合。
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// 返回位掩码。
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// 判断是否包含全部所需权利。
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

impl core::ops::BitOr for HandleRights {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
