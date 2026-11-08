//! 盘符到 HolyFS 卷的注册表。

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::{BlockDevice, DiskSlice};
use hk_block_ipc::ClientDisk;
use hk_fs_holyfs::{
    DocId, HolyError, HolyFs, VolumeId, format_with_volume_id, is_formatted, volume_id_for_drive,
};
use hk_holy_proto::response;
use hk_proto::vfs::{STATUS_EXISTS, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND};

/// 多个卷共享同一个块客户端，避免同一服务上的重复 Frame 映射。
#[derive(Clone)]
pub(crate) struct SharedClient {
    disk: Rc<RefCell<ClientDisk>>,
}

impl SharedClient {
    pub(crate) fn new(disk: ClientDisk) -> Self {
        Self {
            disk: Rc::new(RefCell::new(disk)),
        }
    }
}

impl BlockDevice for SharedClient {
    fn block_size(&self) -> usize {
        self.disk.borrow().block_size()
    }

    fn capacity_blocks(&self) -> u64 {
        self.disk.borrow().capacity_blocks()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, hk_block::BlockError> {
        self.disk.borrow().read_blocks(block, buffer)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, hk_block::BlockError> {
        self.disk.borrow_mut().write_blocks(block, data)
    }

    fn flush(&mut self) -> Result<(), hk_block::BlockError> {
        self.disk.borrow_mut().flush()
    }
}

/// 服务使用的块设备切片。
pub(crate) type ServiceDisk = DiskSlice<SharedClient>;

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
    services: BTreeMap<u8, String>,
}

impl Volumes {
    pub(crate) const fn new() -> Self {
        Self {
            drives: BTreeMap::new(),
            services: BTreeMap::new(),
        }
    }

    /// 记录盘符使用的块服务，防止同一服务被重复接入。
    pub(crate) fn register_service(&mut self, letter: u8, service: &str) {
        self.services.insert(letter, String::from(service));
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

    /// 动态接入一个块服务并注册盘符；未格式化的设备会先格式化。
    pub(crate) fn attach(&mut self, letter: u8, service: &str, endpoint: u64) -> Result<(), u8> {
        if !letter.is_ascii_alphabetic() {
            return Err(STATUS_INVALID);
        }
        if self.drives.contains_key(&letter)
            || self.services.values().any(|existing| existing == service)
        {
            return Err(STATUS_EXISTS);
        }
        let disk = SharedClient::new(ClientDisk::open(service, endpoint).map_err(|_| STATUS_IO)?);
        let blocks = disk.capacity_blocks();
        if blocks == 0 {
            return Err(STATUS_IO);
        }
        let disk = DiskSlice::new(disk, 0, blocks);
        let disk = if is_formatted(&disk) {
            disk
        } else {
            format_with_volume_id(disk, volume_id_for_drive(letter)).map_err(response::status)?
        };
        let volume = HolyFs::mount(disk).map_err(response::status)?;
        self.insert(letter, volume).map_err(response::status)?;
        self.services.insert(letter, String::from(service));
        Ok(())
    }

    /// 刷写并移除盘符。
    pub(crate) fn detach(&mut self, letter: u8) -> Result<(), u8> {
        let mut volume = self.drives.remove(&letter).ok_or(STATUS_NOT_FOUND)?;
        if let Err(error) = volume.sync() {
            self.drives.insert(letter, volume);
            return Err(response::status(error));
        }
        self.services.remove(&letter);
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

/// 解析盘符文本；接受 `J` 与 `J:`。
pub(crate) fn parse_drive(text: &str) -> Option<u8> {
    let letter = text.as_bytes().first().copied()?.to_ascii_uppercase();
    letter.is_ascii_alphabetic().then_some(letter)
}
