//! HolyFS：HolyKernel 原生文档文件系统。
//!
//! 卷以文档为身份、以修订承载内容与集合项、以 COW 元数据树提交。
//! 本 crate 只暴露 HolyFS 原生文档操作。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod compress;
mod crc32c;
mod error;
mod handle;
mod hash;
pub mod ids;
mod store;
mod superblock;
mod tree;
pub mod types;
mod volume;

#[cfg(test)]
mod tests;

use hk_block::BlockDevice;

pub use compress::decompress_into;
pub use error::HolyError;
pub use handle::HolyFs;
pub use ids::{DocId, HandleRights, RevisionId, VolumeId};
pub use superblock::{BLOCK_SIZE, VOLUME_H, VOLUME_I, volume_id_for_drive};
pub use types::{
    DocumentInfo, DocumentKind, DocumentRecord, EntryInfo, EntryRecord, Extent, MaintenanceReport,
    PayloadKind, Quota, RevisionFlags, RevisionInfo, RevisionRecord, SnapshotInfo, StoredContent,
};

/// 格式化块设备为 HolyFS。
pub fn format<D: BlockDevice>(device: D) -> Result<D, HolyError> {
    volume::Volume::format(device)
}

/// 格式化块设备为指定卷身份的 HolyFS。
pub fn format_with_volume_id<D: BlockDevice>(
    device: D,
    volume_id: hk_fs::VolumeId,
) -> Result<D, HolyError> {
    volume::Volume::format_with_volume_id(device, volume_id)
}

/// 判断块设备是否包含可挂载的 HolyFS 超级块。
pub fn is_formatted<D: BlockDevice>(device: &D) -> bool {
    volume::Volume::is_formatted(device)
}
