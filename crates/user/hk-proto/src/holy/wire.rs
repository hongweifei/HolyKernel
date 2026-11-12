//! 线上请求标签、操作码、权利位与记录长度。

use crate::wire::{self, RequestHeader};

/// 请求标签。
pub const HOLY_REQUEST: u64 = crate::labels::HOLY;
/// 响应标签。
pub const HOLY_RESPONSE: u64 = crate::labels::HOLY + 1;

/// 按 DocPath 整文档读取。
pub const OP_READ: u8 = 2;
/// 读取根文档身份。
pub const OP_ROOT: u8 = 5;
/// 在集合中按名字解析。
pub const OP_RESOLVE: u8 = 6;
/// 在集合中创建文档。
pub const OP_CREATE: u8 = 7;
/// 移除集合项。
pub const OP_UNBIND: u8 = 8;
/// 按 DocId 枚举集合。
pub const OP_ENUMERATE_ID: u8 = 9;
/// 按 DocId 查询视图。
pub const OP_DOCUMENT_ID: u8 = 10;
/// 按 DocId 与偏移读取。
pub const OP_READ_ID: u8 = 11;
/// 按 DocId 与偏移写入。
pub const OP_WRITE_ID: u8 = 12;
/// 按 DocId 调整大小。
pub const OP_TRUNCATE_ID: u8 = 13;
/// 按 DocId 枚举修订链。
pub const OP_REVISIONS_ID: u8 = 14;
/// 提交卷写入。
pub const OP_SYNC: u8 = 15;
/// 获取文档句柄令牌。
pub const OP_ACQUIRE: u8 = 16;
/// 释放令牌。
pub const OP_RELEASE: u8 = 17;
/// 通过令牌读取。
pub const OP_READ_TOKEN: u8 = 18;
/// 通过令牌写入。
pub const OP_WRITE_TOKEN: u8 = 19;
/// 通过令牌调整大小。
pub const OP_TRUNCATE_TOKEN: u8 = 20;
/// 通过令牌查询视图。
pub const OP_DOCUMENT_TOKEN: u8 = 21;
/// 收窄令牌权利。
pub const OP_GRANT: u8 = 22;
/// 撤销令牌。
pub const OP_REVOKE: u8 = 23;
/// 把令牌转移到另一个端点。
pub const OP_TRANSFER: u8 = 24;
/// 枚举标签。
pub const OP_TAGS: u8 = 25;
/// 枚举标签成员。
pub const OP_TAG_MEMBERS: u8 = 26;
/// 创建标签。
pub const OP_CREATE_TAG: u8 = 27;
/// 删除标签。
pub const OP_DROP_TAG: u8 = 28;
/// 把文档加入标签。
pub const OP_TAG: u8 = 29;
/// 把文档从标签移除。
pub const OP_UNTAG: u8 = 30;
/// 枚举文档所属标签。
pub const OP_TAGS_OF: u8 = 31;
/// 创建快照。
pub const OP_SNAPSHOT: u8 = 32;
/// 枚举快照。
pub const OP_SNAPSHOTS: u8 = 33;
/// 恢复快照。
pub const OP_RESTORE: u8 = 34;
/// 释放快照。
pub const OP_FORGET: u8 = 35;
/// 查询卷身份、提交代数与卷代。
pub const OP_VOLUME: u8 = 36;
/// 获取卷级令牌。
pub const OP_ACQUIRE_VOLUME: u8 = 37;
/// 释放卷级令牌。
pub const OP_RELEASE_VOLUME: u8 = 38;
/// 收窄卷级令牌权利。
pub const OP_GRANT_VOLUME: u8 = 39;
/// 撤销卷级令牌。
pub const OP_REVOKE_VOLUME: u8 = 40;
/// 按 DocId 与固定修订读取。
pub const OP_READ_REVISION: u8 = 41;
/// 枚举已注册盘符。
pub const OP_DRIVES: u8 = 42;
/// 动态接入块服务并注册盘符。
pub const OP_ATTACH_DRIVE: u8 = 43;
/// 刷写并移除盘符。
pub const OP_DETACH_DRIVE: u8 = 44;
/// 只读校验卷完整性。
pub const OP_CHECK: u8 = 45;
/// 校验并修复卷分配位图。
pub const OP_REPAIR: u8 = 46;
/// 校验并回收泄漏块。
pub const OP_SCRUB: u8 = 47;
/// 封存修订。
pub const OP_SEAL: u8 = 48;
/// 裁剪修订链并回收旧块。
pub const OP_PRUNE: u8 = 49;
/// 显式销毁文档及其修订链。
pub const OP_PURGE: u8 = 50;
/// 查询卷配额。
pub const OP_QUOTA: u8 = 51;
/// 设置卷配额。
pub const OP_SET_QUOTA: u8 = 52;
/// 创建 staging。
pub const OP_STAGE: u8 = 53;
/// 写入 staging 区间。
pub const OP_WRITE_STAGING: u8 = 54;
/// 提交 staging。
pub const OP_COMMIT_STAGING: u8 = 55;
/// 放弃 staging。
pub const OP_ABORT_STAGING: u8 = 56;

/// 内容文档。
pub const KIND_CONTENT: u8 = 0;
/// 集合文档。
pub const KIND_COLLECTION: u8 = 1;

/// 查询视图。
pub const TOKEN_RIGHT_INSPECT: u32 = 1 << 0;
/// 读取内容。
pub const TOKEN_RIGHT_READ: u32 = 1 << 1;
/// 写入内容。
pub const TOKEN_RIGHT_WRITE: u32 = 1 << 2;
/// 集合变更。
pub const TOKEN_RIGHT_BIND: u32 = 1 << 3;
/// 管理操作。
pub const TOKEN_RIGHT_ADMIN: u32 = 1 << 4;
/// 转移令牌。
pub const TOKEN_RIGHT_TRANSFER: u32 = 1 << 5;

/// 当前承载卷的服务名。
pub const DRIVE_SERVICE: &str = "hk-server-holyfs";

/// 解码后的 Holy 请求。
pub struct Request<'a> {
    /// 操作码。
    pub op: u8,
    /// 盘符；0 表示不指定。
    pub drive: u8,
    /// 操作文本：名字或 DocPath，由操作决定。
    pub path: &'a str,
    /// 第一个参数。
    pub arg0: u64,
    /// 第二个参数。
    pub arg1: u64,
    /// 附加数据。
    pub data: &'a [u8],
}

/// 编码 Holy 请求。
pub fn encode_request(
    buffer: &mut [u8],
    op: u8,
    drive: u8,
    path: &str,
    arg0: u64,
    arg1: u64,
    data: &[u8],
) -> Option<usize> {
    wire::encode_request(
        buffer,
        RequestHeader::new(op, drive, arg0, arg1),
        path.as_bytes(),
        data,
    )
}

/// 解码 Holy 请求。
pub fn decode_request(bytes: &[u8]) -> Option<Request<'_>> {
    let request = wire::decode_request(bytes)?;
    Some(Request {
        op: request.header.op,
        drive: request.header.kind,
        path: core::str::from_utf8(request.text).ok()?,
        arg0: request.header.arg0,
        arg1: request.header.arg1,
        data: request.data,
    })
}

/// 请求头长度；布局由 公共帧头 统一，`kind` 承载盘符。
pub const REQUEST_HEADER: usize = crate::wire::REQUEST_HEADER;
/// 请求缓冲上限。
pub const REQUEST_BUFFER: usize = 64 * 1024;
/// 响应缓冲上限。
pub const RESPONSE_BUFFER: usize = 64 * 1024;
/// 集合项头：kind + DocId + 名字长度。
pub(super) const ENTRY_HEADER: usize = 1 + 16 + 1;
/// 文档视图长度。
pub(super) const DOCUMENT_LEN: usize = 1 + 8 + 8 + 8 + 8;
/// 修订视图长度。
pub(super) const REVISION_LEN: usize = 8 + 8 + 8 + 8 + 4;
/// 文档身份编码长度。
pub const DOC_LEN: usize = 16;
/// 修订身份编码长度。
pub(super) const REVISION_ID_LEN: usize = 8;
/// 令牌编码长度。
pub(super) const TOKEN_LEN: usize = 8;
/// 标签头：编号 + 名字长度。
pub(super) const TAG_HEADER: usize = 4 + 1;
/// 快照头：编号、代数、时间、文档数与名字长度。
pub(super) const SNAPSHOT_HEADER: usize = 8 + 8 + 8 + 4 + 1;
/// 卷视图：卷身份、提交代数、卷代与根文档。
pub(super) const VOLUME_LEN: usize = 16 + 8 + 8 + 16;
/// 盘符视图：盘符 + 卷视图。
pub(super) const DRIVE_LEN: usize = 1 + VOLUME_LEN;
/// 维护报告：9 个 u64 统计字段。
pub(super) const MAINTENANCE_LEN: usize = 9 * 8;
/// 配额视图：5 个 u64 限制字段。
pub(super) const QUOTA_LEN: usize = 5 * 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_and_invalid_utf8_requests() {
        for length in 0..32 {
            assert!(decode_request(&[0xFF_u8; 32][..length]).is_none());
        }
        let mut frame = [0_u8; 21];
        frame[2] = 1;
        frame[20] = 0xFF;
        assert!(decode_request(&frame).is_none());
    }
}
