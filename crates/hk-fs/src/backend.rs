//! 文件系统后端契约。
//!
//! 后端以文档身份为入口；路径、fd 与偏移由上层适配。

use alloc::vec::Vec;

use crate::caps::Caps;
use crate::error::Error;
use crate::ids::{DocId, RevisionId};
use crate::types::{DocumentInfo, DocumentKind, EntryInfo, RevisionInfo};

/// Holy 文件系统后端。
pub trait Backend {
    /// 返回根集合文档。
    fn root(&self) -> DocId;

    /// 返回后端能力。
    fn caps(&self) -> Caps;

    /// 读取文档视图。
    fn document(&self, document: DocId) -> Result<DocumentInfo, Error>;

    /// 在集合中按名字解析文档。
    fn resolve(&self, collection: DocId, name: &str) -> Result<DocId, Error>;

    /// 枚举集合项。
    fn enumerate(&self, collection: DocId) -> Result<Vec<EntryInfo>, Error>;

    /// 在集合中创建文档。
    fn create(&mut self, collection: DocId, name: &str, kind: DocumentKind)
    -> Result<DocId, Error>;

    /// 移除集合项，返回被移除的文档身份。
    fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, Error>;

    /// 从文档偏移读取；`revision` 为 `None` 时读取 head。
    fn read(
        &self,
        document: DocId,
        revision: Option<RevisionId>,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error>;

    /// 从文档偏移写入，返回写入字节数。
    fn write(&mut self, document: DocId, offset: u64, data: &[u8]) -> Result<usize, Error>;

    /// 调整文档大小。
    fn truncate(&mut self, document: DocId, size: u64) -> Result<(), Error>;

    /// 枚举修订链。
    fn revisions(&self, document: DocId) -> Result<Vec<RevisionInfo>, Error>;

    /// 把挂起写入提交到稳定存储。
    fn sync(&mut self) -> Result<(), Error> {
        Ok(())
    }

    /// 该文档是否使用上层内容缓存。
    fn cacheable(&self, _document: DocId) -> bool {
        true
    }
}
