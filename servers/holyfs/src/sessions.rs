//! HolyFS 服务的文档与卷级令牌会话。

use hk_fs::{DocSession, DocSessions, VolumeSession, VolumeSessions};

/// 按 `(发送者, 盘符)` 分组的文档与卷级会话。
pub(crate) struct Sessions {
    documents: DocSessions,
    volumes: VolumeSessions,
}

impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            documents: DocSessions::new(),
            volumes: VolumeSessions::new(),
        }
    }

    pub(crate) fn doc(&mut self, sender: u64, drive: u8) -> &mut DocSession {
        self.documents.doc(sender, drive)
    }

    /// 文档级会话表；通用分派直接使用。
    pub(crate) fn documents(&mut self) -> &mut DocSessions {
        &mut self.documents
    }

    pub(crate) fn volume(&mut self, sender: u64, drive: u8) -> &mut VolumeSession {
        self.volumes.volume(sender, drive)
    }

    pub(crate) fn invalidate_documents(&mut self, drive: u8) {
        self.documents.invalidate_drive(drive);
    }
}
