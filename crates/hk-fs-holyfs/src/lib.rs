//! HolyFS：HolyKernel 原生文档文件系统。
//!
//! 卷以文档为身份、以修订承载内容与集合项、以 COW 元数据树提交。
//! VFS 适配层把路径与偏移映射到文档操作；原生 Document API 在后续逐步开放。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod crc32c;
mod error;
pub mod ids;
mod store;
mod superblock;
mod tree;
pub mod types;
mod vfs;
mod volume;

#[cfg(test)]
mod tests;

use hk_block::BlockDevice;

pub use error::HolyError;
pub use ids::{DocId, HandleRights, RevisionId, VolumeId};
pub use superblock::BLOCK_SIZE;
pub use types::{
    DocumentKind, DocumentRecord, EntryRecord, Extent, PayloadKind, RevisionFlags, RevisionRecord,
};
pub use vfs::HolyFs;

/// 格式化块设备为 HolyFS。
pub fn format<D: BlockDevice>(device: D) -> Result<D, HolyError> {
    volume::Volume::format(device)
}
