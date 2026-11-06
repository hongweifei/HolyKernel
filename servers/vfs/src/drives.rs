//! 原生 VFS 的盘符到 Holy Backend 注册表。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_fs::{Backend, DocId};
use hk_proto::holy::VolumeView;

/// 一个盘符上的原生后端。
pub(crate) struct Drive {
    pub(crate) backend: Box<dyn Backend>,
    pub(crate) volume: VolumeView,
    pub(crate) root: DocId,
}

/// 盘符注册表。
pub(crate) struct Drives {
    drives: BTreeMap<u8, Drive>,
}

impl Drives {
    pub(crate) const fn new() -> Self {
        Self {
            drives: BTreeMap::new(),
        }
    }

    pub(crate) fn insert(
        &mut self,
        letter: u8,
        backend: Box<dyn Backend>,
        volume: VolumeView,
        root: DocId,
    ) -> bool {
        if !letter.is_ascii_alphabetic() || self.drives.contains_key(&letter) {
            return false;
        }
        self.drives.insert(
            letter,
            Drive {
                backend,
                volume,
                root,
            },
        );
        true
    }

    pub(crate) fn get_mut(&mut self, letter: u8) -> Option<&mut Drive> {
        self.drives.get_mut(&letter)
    }

    pub(crate) fn list(&self) -> Vec<(u8, VolumeView)> {
        self.drives
            .iter()
            .map(|(letter, drive)| (*letter, drive.volume))
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.drives.is_empty()
    }
}
