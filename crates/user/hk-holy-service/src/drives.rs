//! 盘符到 Holy Backend 的注册表。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_fs::{Backend, DocId};
use hk_proto::holy::{DocId as WireDocId, VolumeView};

/// 进程内后端（ext4、tmpfs、devfs、procfs）的卷标识基址。
pub const LOCAL_VOLUME_BASE: u128 = 0x484F_4C59_4653_0200;

/// 一个盘符上的后端。
pub struct Drive {
    /// 文档后端。
    pub backend: Box<dyn Backend>,
    /// 盘符对外的卷视图。
    pub volume: VolumeView,
    /// 根文档。
    pub root: DocId,
}

/// 盘符注册表。
pub struct Drives {
    drives: BTreeMap<u8, Drive>,
}

impl Drives {
    /// 构造空注册表。
    pub const fn new() -> Self {
        Self {
            drives: BTreeMap::new(),
        }
    }

    /// 注册一个后端；盘符非法或已存在时返回 `false`。
    pub fn insert(
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

    /// 以进程内后端的标准卷视图注册盘符。
    pub fn insert_local<P: Backend + 'static>(&mut self, letter: u8, backend: P) -> bool {
        let root = backend.root();
        let volume = VolumeView {
            volume: LOCAL_VOLUME_BASE + letter as u128,
            generation: 0,
            epoch: 0,
            root: WireDocId::new(root.get()),
        };
        self.insert(letter, Box::new(backend), volume, root)
    }

    /// 返回盘符上的后端。
    pub fn get_mut(&mut self, letter: u8) -> Option<&mut Drive> {
        self.drives.get_mut(&letter)
    }

    /// 列出全部盘符与卷视图。
    pub fn list(&self) -> Vec<(u8, VolumeView)> {
        self.drives
            .iter()
            .map(|(letter, drive)| (*letter, drive.volume))
            .collect()
    }

    /// 是否没有任何盘符。
    pub fn is_empty(&self) -> bool {
        self.drives.is_empty()
    }
}

impl Default for Drives {
    fn default() -> Self {
        Self::new()
    }
}
