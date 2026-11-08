//! HolyFS 服务的文档与卷级令牌会话。

use hk_fs::{DocSession, DocSessions, Error, VolumeSession, VolumeSessions};

/// 按 `(发送者, 盘符)` 分组的文档与卷级会话。
#[cfg(target_os = "none")]
pub(crate) struct Sessions {
    documents: DocSessions,
    volumes: VolumeSessions,
}

#[cfg(target_os = "none")]
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

    pub(crate) fn volume(&mut self, sender: u64, drive: u8) -> &mut VolumeSession {
        self.volumes.volume(sender, drive)
    }

    pub(crate) fn transfer(
        &mut self,
        from: u64,
        to: u64,
        token: u64,
        drive: u8,
    ) -> Result<u64, Error> {
        self.documents.transfer(from, to, token, drive)
    }

    pub(crate) fn invalidate_documents(&mut self, drive: u8) {
        self.documents.invalidate_drive(drive);
    }
}
