//! Holy 文档客户端：收发缓冲按需分配。

use alloc::vec::Vec;

use hk_proto::holy::{
    DOC_LEN, DRIVE_SERVICE, DocId, DocumentKind, DocumentView, HOLY_REQUEST, HOLY_RESPONSE,
    MaintenanceView, OP_ABORT_STAGING, OP_ACQUIRE, OP_ACQUIRE_VOLUME, OP_ATTACH_DRIVE, OP_CHECK,
    OP_COMMIT_STAGING, OP_CREATE, OP_CREATE_TAG, OP_DETACH_DRIVE, OP_DOCUMENT_ID,
    OP_DOCUMENT_TOKEN, OP_DRIVES, OP_DROP_TAG, OP_ENUMERATE_ID, OP_FORGET, OP_GRANT,
    OP_GRANT_VOLUME, OP_PRUNE, OP_PURGE, OP_QUOTA, OP_READ, OP_READ_ID, OP_READ_REVISION,
    OP_READ_TOKEN, OP_RELEASE, OP_RELEASE_VOLUME, OP_REPAIR, OP_RESOLVE, OP_RESTORE,
    OP_REVISIONS_ID, OP_REVOKE, OP_REVOKE_VOLUME, OP_ROOT, OP_SCRUB, OP_SEAL, OP_SET_QUOTA,
    OP_SNAPSHOT, OP_SNAPSHOTS, OP_STAGE, OP_SYNC, OP_TAG, OP_TAG_MEMBERS, OP_TAGS, OP_TAGS_OF,
    OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND, OP_UNTAG, OP_VOLUME, OP_WRITE_ID,
    OP_WRITE_STAGING, OP_WRITE_TOKEN, QuotaView, REQUEST_BUFFER, REQUEST_HEADER, RESPONSE_BUFFER,
    RevisionId, VolumeView, decode_doc_id, decode_document, decode_maintenance, decode_quota,
    decode_revision_id, decode_token, decode_u32, decode_u64, decode_volume, encode_quota,
    encode_request, kind_code,
};
use hk_proto::status::{STATUS_ERROR, STATUS_INVALID, STATUS_IO, STATUS_OK, decode_response};
use hk_server_api::{recv_data_from, send_data, service_endpoint};

/// Holy 文档客户端；缓冲在首次请求时分配。
pub struct HolyClient {
    own_endpoint: u64,
    service: &'static str,
    endpoint: u64,
    drive: u8,
    request: Vec<u8>,
    response: Vec<u8>,
}

impl HolyClient {
    /// 构造客户端；`own_endpoint` 是本任务端点。
    pub const fn new(own_endpoint: u64) -> Self {
        Self::new_with_service(own_endpoint, DRIVE_SERVICE)
    }

    /// 构造指向指定服务的客户端。
    pub const fn new_with_service(own_endpoint: u64, service: &'static str) -> Self {
        Self {
            own_endpoint,
            service,
            endpoint: 0,
            drive: b'H',
            request: Vec::new(),
            response: Vec::new(),
        }
    }

    fn ensure_buffers(&mut self) {
        if self.request.len() < REQUEST_BUFFER {
            self.request.resize(REQUEST_BUFFER, 0);
        }
        if self.response.len() < RESPONSE_BUFFER {
            self.response.resize(RESPONSE_BUFFER, 0);
        }
    }

    /// 选择后续请求使用的盘符。
    pub fn select_drive(&mut self, drive: &str) -> Result<(), u8> {
        let letter = drive.as_bytes().first().copied().ok_or(STATUS_INVALID)?;
        let letter = letter.to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() {
            return Err(STATUS_INVALID);
        }
        self.drive = letter;
        Ok(())
    }

    /// 返回当前盘符。
    pub const fn drive(&self) -> u8 {
        self.drive
    }

    /// 直接设置盘符；调用方保证是 A-Z。
    pub fn set_drive(&mut self, drive: u8) {
        self.drive = drive.to_ascii_uppercase();
    }

    /// 枚举已注册盘符，返回编码后的盘符字节数。
    pub fn drives(&mut self) -> Result<&[u8], u8> {
        let length = self.exchange(OP_DRIVES, "", 0, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 动态接入块服务并注册盘符；`token` 是当前盘符的 ADMIN 卷令牌。
    pub fn attach_drive(&mut self, token: u64, drive: &str, service: &str) -> Result<(), u8> {
        self.exchange(OP_ATTACH_DRIVE, drive, token, 0, service.as_bytes())
            .map(|_| ())
    }

    /// 刷写并移除盘符；`token` 是当前盘符的 ADMIN 卷令牌。
    pub fn detach_drive(&mut self, token: u64, drive: &str) -> Result<(), u8> {
        self.exchange(OP_DETACH_DRIVE, drive, token, 0, &[])
            .map(|_| ())
    }

    /// 整文档读取，返回内容字节数。
    pub fn read(&mut self, path: &str, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_READ, path, 0, 0, &[])?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 读取根集合文档身份。
    pub fn root(&mut self) -> Result<DocId, u8> {
        let length = self.exchange(OP_ROOT, "", 0, 0, &[])?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 在集合中按名字解析文档。
    pub fn resolve(&mut self, collection: DocId, name: &str) -> Result<DocId, u8> {
        let length = self.exchange(OP_RESOLVE, name, 0, 0, &collection.to_le_bytes())?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 在集合中创建文档。
    pub fn create(
        &mut self,
        collection: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, u8> {
        let length = self.exchange(
            OP_CREATE,
            name,
            u64::from(kind_code(kind)),
            0,
            &collection.to_le_bytes(),
        )?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 移除集合项。
    pub fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, u8> {
        let length = self.exchange(OP_UNBIND, name, 0, 0, &collection.to_le_bytes())?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份枚举集合。
    pub fn enumerate_id(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_ENUMERATE_ID, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 按文档身份查询视图。
    pub fn document_id(&mut self, document: DocId) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT_ID, "", 0, 0, &document.to_le_bytes())?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份与偏移读取。
    pub fn read_id(&mut self, document: DocId, offset: u64, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(
            OP_READ_ID,
            "",
            offset,
            out.len() as u64,
            &document.to_le_bytes(),
        )?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 按文档身份与固定修订读取。
    pub fn read_revision(
        &mut self,
        document: DocId,
        revision: RevisionId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, u8> {
        let mut payload = [0_u8; DOC_LEN + 8];
        payload[..DOC_LEN].copy_from_slice(&document.to_le_bytes());
        payload[DOC_LEN..].copy_from_slice(&revision.to_le_bytes());
        let length = self.exchange(OP_READ_REVISION, "", offset, out.len() as u64, &payload)?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 按文档身份与偏移写入。
    pub fn write_id(
        &mut self,
        document: DocId,
        offset: u64,
        data: &[u8],
    ) -> Result<RevisionId, u8> {
        let length = self.exchange_with_doc(OP_WRITE_ID, offset, 0, document, data)?;
        decode_revision_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份调整大小。
    pub fn truncate_id(&mut self, document: DocId, size: u64) -> Result<(), u8> {
        self.exchange(OP_TRUNCATE_ID, "", size, 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 按文档身份枚举修订链。
    pub fn revisions_id(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_REVISIONS_ID, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 提交卷写入。
    pub fn sync(&mut self) -> Result<(), u8> {
        self.exchange(OP_SYNC, "", 0, 0, &[]).map(|_| ())
    }

    /// 查询卷视图。
    pub fn volume(&mut self) -> Result<VolumeView, u8> {
        let length = self.exchange(OP_VOLUME, "", 0, 0, &[])?;
        decode_volume(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 获取卷级令牌。
    pub fn acquire_volume(&mut self, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(OP_ACQUIRE_VOLUME, "", u64::from(rights), 0, &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 释放卷级令牌。
    pub fn release_volume(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_RELEASE_VOLUME, "", token, 0, &[])
            .map(|_| ())
    }

    /// 收窄卷级令牌权利，返回子令牌。
    pub fn grant_volume(&mut self, token: u64, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(OP_GRANT_VOLUME, "", token, u64::from(rights), &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 撤销卷级令牌及其子令牌。
    pub fn revoke_volume(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_REVOKE_VOLUME, "", token, 0, &[])
            .map(|_| ())
    }

    /// 创建快照，返回快照编号。
    pub fn snapshot(&mut self, token: u64, name: &str) -> Result<u64, u8> {
        let length = self.exchange(OP_SNAPSHOT, name, token, 0, &[])?;
        decode_u64(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 枚举快照，返回编码后的快照字节数。
    pub fn snapshots(&mut self, token: u64) -> Result<&[u8], u8> {
        let length = self.exchange(OP_SNAPSHOTS, "", token, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 恢复快照，返回新的提交代数。
    pub fn restore(&mut self, token: u64, snapshot: u64) -> Result<u64, u8> {
        let length = self.exchange(OP_RESTORE, "", token, snapshot, &[])?;
        decode_u64(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 释放快照固定。
    pub fn forget(&mut self, token: u64, snapshot: u64) -> Result<(), u8> {
        self.exchange(OP_FORGET, "", token, snapshot, &[])
            .map(|_| ())
    }

    /// 只读校验卷完整性。
    pub fn check(&mut self, token: u64) -> Result<MaintenanceView, u8> {
        let length = self.exchange(OP_CHECK, "", token, 0, &[])?;
        decode_maintenance(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 校验并修复分配位图。
    pub fn repair(&mut self, token: u64) -> Result<MaintenanceView, u8> {
        let length = self.exchange(OP_REPAIR, "", token, 0, &[])?;
        decode_maintenance(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 校验并回收泄漏块。
    pub fn scrub(&mut self, token: u64) -> Result<MaintenanceView, u8> {
        let length = self.exchange(OP_SCRUB, "", token, 0, &[])?;
        decode_maintenance(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 封存修订。
    pub fn seal(&mut self, token: u64, revision: RevisionId) -> Result<(), u8> {
        self.exchange(OP_SEAL, "", token, 0, &revision.to_le_bytes())
            .map(|_| ())
    }

    /// 裁剪文档修订链并回收旧块。
    pub fn prune(
        &mut self,
        token: u64,
        document: DocId,
        keep_last: u64,
    ) -> Result<MaintenanceView, u8> {
        let length = self.exchange(OP_PRUNE, "", token, keep_last, &document.to_le_bytes())?;
        decode_maintenance(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 显式销毁文档及其修订链。
    pub fn purge(&mut self, token: u64, document: DocId) -> Result<(), u8> {
        self.exchange(OP_PURGE, "", token, 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 查询卷配额。
    pub fn quota(&mut self, token: u64) -> Result<QuotaView, u8> {
        let length = self.exchange(OP_QUOTA, "", token, 0, &[])?;
        decode_quota(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 设置卷配额。
    pub fn set_quota(&mut self, token: u64, quota: QuotaView) -> Result<(), u8> {
        let mut bytes = [0_u8; 40];
        encode_quota(&mut bytes, quota).ok_or(STATUS_INVALID)?;
        self.exchange(OP_SET_QUOTA, "", token, 0, &bytes)
            .map(|_| ())
    }

    /// 创建 staging，返回 staging 编号。
    pub fn stage(&mut self, document: DocId, expected: Option<RevisionId>) -> Result<u64, u8> {
        let length = self.exchange(
            OP_STAGE,
            "",
            expected.map_or(0, RevisionId::get),
            0,
            &document.to_le_bytes(),
        )?;
        decode_u64(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 写入 staging 区间。
    pub fn write_staging(&mut self, staging: u64, offset: u64, data: &[u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_WRITE_STAGING, "", staging, offset, data)?;
        decode_u64(&self.response[3..3 + length])
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(STATUS_INVALID)
    }

    /// 提交 staging，返回新 head 修订。
    pub fn commit_staging(
        &mut self,
        staging: u64,
        expected: Option<RevisionId>,
    ) -> Result<RevisionId, u8> {
        let length = self.exchange(
            OP_COMMIT_STAGING,
            "",
            staging,
            expected.map_or(0, RevisionId::get),
            &[],
        )?;
        decode_revision_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 放弃 staging。
    pub fn abort_staging(&mut self, staging: u64) -> Result<(), u8> {
        self.exchange(OP_ABORT_STAGING, "", staging, 0, &[])
            .map(|_| ())
    }

    /// 获取文档令牌。
    pub fn acquire(&mut self, document: DocId, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(
            OP_ACQUIRE,
            "",
            u64::from(rights),
            0,
            &document.to_le_bytes(),
        )?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 释放令牌。
    pub fn release(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_RELEASE, "", token, 0, &[]).map(|_| ())
    }

    /// 收窄令牌权利，返回子令牌。
    pub fn grant(&mut self, token: u64, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(OP_GRANT, "", token, u64::from(rights), &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 撤销令牌及其子令牌。
    pub fn revoke(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_REVOKE, "", token, 0, &[]).map(|_| ())
    }

    /// 把令牌转移到目标端点。
    pub fn transfer(&mut self, token: u64, target: u64) -> Result<u64, u8> {
        let length = self.exchange(OP_TRANSFER, "", token, target, &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 枚举标签，返回编码后的标签字节数。
    pub fn tags(&mut self) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAGS, "", 0, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 枚举标签成员，返回编码后的文档身份字节数。
    pub fn tag_members(&mut self, tag: u32) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAG_MEMBERS, "", u64::from(tag), 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 创建标签，返回标签编号。
    pub fn create_tag(&mut self, name: &str) -> Result<u32, u8> {
        let length = self.exchange(OP_CREATE_TAG, name, 0, 0, &[])?;
        decode_u32(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 删除标签。
    pub fn drop_tag(&mut self, tag: u32) -> Result<(), u8> {
        self.exchange(OP_DROP_TAG, "", u64::from(tag), 0, &[])
            .map(|_| ())
    }

    /// 把文档加入标签。
    pub fn tag(&mut self, document: DocId, tag: u32) -> Result<(), u8> {
        self.exchange(OP_TAG, "", u64::from(tag), 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 把文档从标签移除。
    pub fn untag(&mut self, document: DocId, tag: u32) -> Result<(), u8> {
        self.exchange(OP_UNTAG, "", u64::from(tag), 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 枚举文档所属标签，返回编码后的标签编号字节数。
    pub fn tags_of(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAGS_OF, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 通过令牌读取。
    pub fn read_token(&mut self, token: u64, offset: u64, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(
            OP_READ_TOKEN,
            "",
            token,
            offset,
            &(out.len() as u64).to_le_bytes(),
        )?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 通过令牌写入。
    pub fn write_token(&mut self, token: u64, offset: u64, data: &[u8]) -> Result<RevisionId, u8> {
        let length = self.exchange(OP_WRITE_TOKEN, "", token, offset, data)?;
        decode_revision_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 通过令牌调整大小。
    pub fn truncate_token(&mut self, token: u64, size: u64) -> Result<(), u8> {
        self.exchange(OP_TRUNCATE_TOKEN, "", token, size, &[])
            .map(|_| ())
    }

    /// 通过令牌查询视图。
    pub fn document_token(&mut self, token: u64) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT_TOKEN, "", token, 0, &[])?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    fn exchange(
        &mut self,
        op: u8,
        path: &str,
        arg0: u64,
        arg1: u64,
        data: &[u8],
    ) -> Result<usize, u8> {
        self.ensure_buffers();
        let Some(length) =
            encode_request(&mut self.request, op, self.drive, path, arg0, arg1, data)
        else {
            return Err(STATUS_INVALID);
        };
        self.submit(length)
    }

    fn exchange_with_doc(
        &mut self,
        op: u8,
        arg0: u64,
        arg1: u64,
        document: DocId,
        payload: &[u8],
    ) -> Result<usize, u8> {
        self.ensure_buffers();
        let header = REQUEST_HEADER;
        let end = header + DOC_LEN + payload.len();
        if end > self.request.len() {
            return Err(STATUS_INVALID);
        }
        self.request[0] = op;
        self.request[1] = self.drive;
        self.request[2..4].copy_from_slice(&0_u16.to_le_bytes());
        self.request[4..12].copy_from_slice(&arg0.to_le_bytes());
        self.request[12..20].copy_from_slice(&arg1.to_le_bytes());
        self.request[REQUEST_HEADER..REQUEST_HEADER + DOC_LEN]
            .copy_from_slice(&document.to_le_bytes());
        self.request[REQUEST_HEADER + DOC_LEN..end].copy_from_slice(payload);
        self.submit(end)
    }

    fn submit(&mut self, length: usize) -> Result<usize, u8> {
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_IO);
        }
        self.endpoint = endpoint;
        if send_data(self.endpoint, HOLY_REQUEST, &self.request[..length]) == u64::MAX {
            return Err(STATUS_IO);
        }
        let (label, _word, _sender, length) =
            recv_data_from(self.own_endpoint, self.endpoint, &mut self.response);
        if label != HOLY_RESPONSE {
            return Err(STATUS_IO);
        }
        if length < 3 {
            return Err(STATUS_ERROR);
        }
        let response = decode_response(&self.response[..length]).ok_or(STATUS_INVALID)?;
        if response.status != STATUS_OK {
            return Err(response.status);
        }
        Ok(response.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_stays_small_and_allocates_lazily() {
        let mut client = HolyClient::new(1);
        assert!(core::mem::size_of::<HolyClient>() < 256);
        assert!(client.request.is_empty() && client.response.is_empty());
        client.ensure_buffers();
        assert_eq!(client.request.len(), REQUEST_BUFFER);
        assert_eq!(client.response.len(), RESPONSE_BUFFER);
    }
}
