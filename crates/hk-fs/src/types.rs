//! Holy 文档视图与类型。

use alloc::string::String;

use crate::ids::{DocId, RevisionId};

/// 文档类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// 内容文档。
    Content,
    /// 集合文档。
    Collection,
}

/// 修订标志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionFlags(u32);

impl RevisionFlags {
    /// 空标志。
    pub const NONE: Self = Self(0);
    /// 修订已封存，不参与自动回收。
    pub const SEALED: Self = Self(1 << 0);
    /// 内容经过压缩。
    pub const COMPRESSED: Self = Self(1 << 1);
    /// 内容经过加密。
    pub const ENCRYPTED: Self = Self(1 << 2);

    /// 构造标志集合。
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// 返回位掩码。
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// 判断是否包含标志。
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
}

/// 文档视图：身份、类型与当前修订。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentInfo {
    /// 文档身份。
    pub id: DocId,
    /// 文档类型。
    pub kind: DocumentKind,
    /// 当前 head 修订。
    pub head: RevisionId,
    /// head 修订的父修订。
    pub parent: Option<RevisionId>,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间；0 表示时钟尚未写入。
    pub moment: u64,
}

/// 集合视图：名字、文档身份与类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInfo {
    /// 集合内名字。
    pub name: String,
    /// 目标文档。
    pub document: DocId,
    /// 文档类型。
    pub kind: DocumentKind,
}

/// 修订视图：身份、历史与内容度量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionInfo {
    /// 修订身份。
    pub id: RevisionId,
    /// 父修订。
    pub parent: Option<RevisionId>,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间。
    pub moment: u64,
    /// 修订标志。
    pub flags: RevisionFlags,
}
