//! 线上文档身份、修订身份与文档类型。

use core::fmt;

/// 线上文档身份；与原生 `DocId` 同构，不依赖分配。
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

/// 线上修订身份。
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

/// 文档类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// 内容文档。
    Content,
    /// 集合文档。
    Collection,
}
