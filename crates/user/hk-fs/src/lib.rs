//! HolyKernel 文件系统契约。
//!
//! 命名空间以文档身份 `DocId`、集合关系与修订为核心；具体文件系统实现
//! [`Backend`]，路径、fd 与偏移由上层适配。

#![no_std]

extern crate alloc;

pub mod backend;
pub mod caps;
pub mod error;
pub mod ids;
pub mod token;
pub mod types;

pub use backend::Backend;
pub use caps::Caps;
pub use error::Error;
pub use ids::{DocId, HandleRights, RevisionId, VolumeId};
pub use token::{Token, TokenTable, VolumeToken, VolumeTokenTable};
pub use types::{
    DocumentInfo, DocumentKind, EntryInfo, RevisionFlags, RevisionInfo, SnapshotInfo, TagInfo,
};
