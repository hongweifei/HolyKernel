//! HolyFS：HolyKernel 原生文档文件系统。
//!
//! 卷以文档为身份、以修订承载内容与集合项、以 COW 元数据树提交。
//! 本 crate 只暴露 HolyFS 原生文档操作。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod crc32c;
mod error;
mod handle;
pub mod ids;
mod store;
mod superblock;
mod tree;
pub mod types;
mod volume;

#[cfg(test)]
mod tests;

use hk_block::BlockDevice;

pub use error::HolyError;
pub use handle::HolyFs;
pub use ids::{DocId, HandleRights, RevisionId, VolumeId};
pub use superblock::BLOCK_SIZE;
pub use types::{
    DocumentInfo, DocumentKind, DocumentRecord, EntryInfo, EntryRecord, Extent, PayloadKind,
    RevisionFlags, RevisionInfo, RevisionRecord,
};

/// 格式化块设备为 HolyFS。
pub fn format<D: BlockDevice>(device: D) -> Result<D, HolyError> {
    volume::Volume::format(device)
}
