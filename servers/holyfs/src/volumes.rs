//! 盘符到 HolyFS 卷的注册表。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_block::DiskSlice;
use hk_block::ipc::ClientDisk;
use hk_fs_holyfs::{DocId, HolyError, HolyFs, VolumeId};

/// 服务使用的块设备切片。
pub(crate) type ServiceDisk = DiskSlice<ClientDisk>;

/// 已注册卷的视图。
pub(crate) struct VolumeInfo {
    pub(crate) letter: u8,
    pub(crate) volume: VolumeId,
    pub(crate) generation: u64,
    pub(crate) epoch: u64,
    pub(crate) root: DocId,
}

/// 盘符到卷的注册表。
pub(crate) struct Volumes {
    drives: BTreeMap<u8, HolyFs<ServiceDisk>>,
}

impl Volumes {
    pub(crate) const fn new() -> Self {
        Self {
            drives: BTreeMap::new(),
        }
    }

    /// 注册盘符；盘符必须是 A-Z，且不能重复。
    pub(crate) fn insert(
        &mut self,
        letter: u8,
        volume: HolyFs<ServiceDisk>,
    ) -> Result<(), HolyError> {
        if !letter.is_ascii_alphabetic() {
            return Err(HolyError::Invalid);
        }
        if self.drives.contains_key(&letter) {
            return Err(HolyError::Exists);
        }
        self.drives.insert(letter, volume);
        Ok(())
    }

    /// 按盘符取卷。
    pub(crate) fn get_mut(&mut self, letter: u8) -> Option<&mut HolyFs<ServiceDisk>> {
        self.drives.get_mut(&letter)
    }

    /// 枚举已注册盘符。
    pub(crate) fn list(&self) -> Vec<VolumeInfo> {
        self.drives
            .iter()
            .map(|(letter, volume)| VolumeInfo {
                letter: *letter,
                volume: volume.volume_id(),
                generation: volume.generation(),
                epoch: volume.epoch(),
                root: volume.root(),
            })
            .collect()
    }
}
