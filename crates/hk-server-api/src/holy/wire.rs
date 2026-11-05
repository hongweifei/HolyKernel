//! 线上请求标签、操作码、权利位与记录长度。

/// 请求标签。
pub const HOLY_REQUEST: u64 = 0x800;
/// 响应标签。
pub const HOLY_RESPONSE: u64 = 0x801;

/// 按 DocPath 枚举集合。
pub const OP_ENUMERATE: u8 = 1;
/// 按 DocPath 整文档读取。
pub const OP_READ: u8 = 2;
/// 按 DocPath 查询文档视图。
pub const OP_DOCUMENT: u8 = 3;
/// 按 DocPath 枚举修订链。
pub const OP_REVISIONS: u8 = 4;
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

/// 内容文档。
pub const KIND_CONTENT: u8 = 0;
/// 集合文档。
pub const KIND_COLLECTION: u8 = 1;

/// 查询视图。
pub const RIGHT_INSPECT: u32 = 1 << 0;
/// 读取内容。
pub const RIGHT_READ: u32 = 1 << 1;
/// 写入内容。
pub const RIGHT_WRITE: u32 = 1 << 2;
/// 集合变更。
pub const RIGHT_BIND: u32 = 1 << 3;
/// 管理操作。
pub const RIGHT_ADMIN: u32 = 1 << 4;
/// 转移令牌。
pub const RIGHT_TRANSFER: u32 = 1 << 5;

/// 当前承载卷的服务名。
pub const DRIVE_SERVICE: &str = "hk-server-vfs";

/// 请求缓冲上限。
pub(super) const REQUEST: usize = 64 * 1024;
/// 响应缓冲上限。
pub(super) const RESPONSE: usize = 64 * 1024;
/// 集合项头：kind + DocId + 名字长度。
pub(super) const ENTRY_HEADER: usize = 1 + 16 + 1;
/// 文档视图长度。
pub(super) const DOCUMENT_LEN: usize = 1 + 8 + 8 + 8 + 8;
/// 修订视图长度。
pub(super) const REVISION_LEN: usize = 8 + 8 + 8 + 8 + 4;
/// 文档身份编码长度。
pub(super) const DOC_LEN: usize = 16;
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
