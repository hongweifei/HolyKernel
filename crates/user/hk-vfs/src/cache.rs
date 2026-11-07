//! 文档页缓存：按 `(实例, DocId, 修订)` 缓存内容页。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;

use hk_fs::{Backend, DocId, Error as FsError, RevisionId};

use crate::vfs::VfsError;

/// 页大小。
pub const PAGE_SIZE: usize = 4096;

type PageKey = (usize, DocId, RevisionId, u64);
type Page = Box<[u8; PAGE_SIZE]>;

/// 默认缓存容量（页）。
pub const DEFAULT_CAPACITY: usize = 64;

struct Entry {
    page: Page,
    last_used: u64,
}

/// 页缓存。
pub struct PageCache {
    pages: BTreeMap<PageKey, Entry>,
    capacity: usize,
    clock: u64,
}

impl PageCache {
    /// 构造默认容量的空缓存。
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// 以指定容量构造空缓存。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            capacity: capacity.max(1),
            clock: 0,
        }
    }

    /// 清空全部缓存。
    pub fn clear(&mut self) {
        self.pages.clear();
    }

    /// 使某文档的全部修订页失效。
    pub fn invalidate(&mut self, instance: usize, document: DocId) {
        self.pages
            .retain(|(cached_instance, cached_document, _, _), _| {
                *cached_instance != instance || *cached_document != document
            });
    }

    /// 返回缓存页数。
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// 判断缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// 从文档读取，未命中时从后端加载整页。
    #[allow(clippy::too_many_arguments)]
    pub fn read<B: Backend + ?Sized>(
        &mut self,
        backend: &B,
        instance: usize,
        document: DocId,
        revision: RevisionId,
        size: u64,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if offset >= size {
            return Ok(0);
        }
        let available = usize::try_from(size - offset).unwrap_or(usize::MAX);
        let limit = buffer.len().min(available);
        // 大块顺序读直接走后端，避免逐页 IPC；代价是不填充页缓存。
        if limit >= PAGE_SIZE * 4 && offset.is_multiple_of(PAGE_SIZE as u64) {
            return backend
                .read(document, Some(revision), offset, &mut buffer[..limit])
                .map_err(map_error);
        }
        let mut done = 0;

        while done < limit {
            let position = offset + done as u64;
            let index = position / PAGE_SIZE as u64;
            let within = (position % PAGE_SIZE as u64) as usize;
            let page = self.load(backend, instance, document, revision, index)?;
            let length = (PAGE_SIZE - within).min(limit - done);
            buffer[done..done + length].copy_from_slice(&page[within..within + length]);
            done += length;
        }
        Ok(done)
    }

    /// 写入文档；写穿透后端并使该文档的缓存失效。
    pub fn write<B: Backend + ?Sized>(
        &mut self,
        backend: &mut B,
        instance: usize,
        document: DocId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        if data.is_empty() {
            return Ok(0);
        }
        let written = backend.write(document, offset, data).map_err(map_error)?;
        self.invalidate(instance, document);
        Ok(written)
    }

    fn load<B: Backend + ?Sized>(
        &mut self,
        backend: &B,
        instance: usize,
        document: DocId,
        revision: RevisionId,
        index: u64,
    ) -> Result<&mut Page, VfsError> {
        let key = (instance, document, revision, index);
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;

        if let Some(entry) = self.pages.get_mut(&key) {
            entry.last_used = clock;
        } else {
            if self.pages.len() >= self.capacity
                && let Some(oldest) = self
                    .pages
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| *key)
            {
                self.pages.remove(&oldest);
            }

            let mut page = Box::new([0_u8; PAGE_SIZE]);
            backend
                .read(
                    document,
                    Some(revision),
                    index * PAGE_SIZE as u64,
                    &mut page[..],
                )
                .map_err(map_error)?;
            self.pages.insert(
                key,
                Entry {
                    page,
                    last_used: clock,
                },
            );
        }
        self.pages
            .get_mut(&key)
            .map(|entry| &mut entry.page)
            .ok_or(VfsError::Backend)
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new()
    }
}

fn map_error(error: FsError) -> VfsError {
    match error {
        FsError::NotFound => VfsError::NotFound,
        FsError::NotDirectory => VfsError::NotDirectory,
        FsError::IsDirectory => VfsError::IsDirectory,
        FsError::Exists => VfsError::AlreadyExists,
        FsError::ReadOnly | FsError::Refused => VfsError::PermissionDenied,
        FsError::Invalid | FsError::Stale => VfsError::InvalidOffset,
        _ => VfsError::Backend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathObject;
    use crate::Tmpfs;
    use crate::vfs::{OpenFlags, Vfs};
    use hk_fs::{Backend, DocumentKind};

    fn writable() -> (PathObject<Tmpfs>, DocId) {
        let mut fs = PathObject::new(Tmpfs::new());
        let root = fs.root();
        let document = fs
            .create(root, "file", DocumentKind::Content)
            .expect("创建");
        (fs, document)
    }

    #[test]
    fn caches_pages_across_reads() {
        let (mut fs, document) = writable();
        fs.write(document, 0, b"hello").expect("写入");
        let info = fs.document(document).expect("视图");
        let revision = info.head;
        let size = info.size;

        let mut cache = PageCache::new();
        let mut buffer = [0_u8; 5];
        assert_eq!(
            cache
                .read(&fs, 0, document, revision, size, 0, &mut buffer)
                .expect("读"),
            5
        );
        assert_eq!(&buffer, b"hello");
        assert_eq!(cache.len(), 1);

        let mut tail = [0_u8; 3];
        assert_eq!(
            cache
                .read(&fs, 0, document, revision, size, 2, &mut tail)
                .expect("读"),
            3
        );
        assert_eq!(&tail, b"llo");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn writes_through_and_invalidates_cache() {
        let (mut fs, document) = writable();
        let mut cache = PageCache::new();
        cache.write(&mut fs, 0, document, 0, b"abc").expect("写");
        assert_eq!(fs.document(document).expect("元数据").size, 3);

        let info = fs.document(document).expect("视图");
        let revision = info.head;
        let mut buffer = [0_u8; 3];
        assert_eq!(
            cache
                .read(&fs, 0, document, revision, info.size, 0, &mut buffer)
                .expect("读"),
            3
        );
        assert_eq!(&buffer, b"abc");
    }

    #[test]
    fn bulk_reads_and_writes_bypass_cache() {
        let (mut fs, document) = writable();
        let chunk = [0x5A_u8; PAGE_SIZE * 8];
        let mut cache = PageCache::new();
        cache
            .write(&mut fs, 0, document, 0, &chunk)
            .expect("批量写");
        assert!(cache.is_empty());
        let info = fs.document(document).expect("视图");
        let revision = info.head;
        let mut buffer = [0_u8; PAGE_SIZE * 8];
        assert_eq!(
            cache
                .read(&fs, 0, document, revision, info.size, 0, &mut buffer)
                .expect("批量读"),
            chunk.len()
        );
        assert_eq!(buffer, chunk);
        assert!(cache.is_empty());
    }

    #[test]
    fn vfs_reads_use_cache() {
        let mut vfs = Vfs::new_path(Tmpfs::new());
        let handle = vfs.open("/file", OpenFlags::CREATE).expect("打开");
        vfs.write(handle, b"cached").expect("写");
        vfs.close(handle).expect("关闭");

        let handle = vfs.open("/file", OpenFlags::READ_ONLY).expect("打开");
        let mut buffer = [0_u8; 6];
        assert_eq!(vfs.read(handle, &mut buffer).expect("读"), 6);
        assert_eq!(&buffer, b"cached");
    }

    #[test]
    fn evicts_least_recently_used_pages() {
        let (mut fs, document) = writable();
        let first = [1_u8; PAGE_SIZE];
        let second = [2_u8; PAGE_SIZE];
        fs.write(document, 0, &first).expect("写第一页");
        fs.write(document, PAGE_SIZE as u64, &second)
            .expect("写第二页");
        let info = fs.document(document).expect("视图");
        let revision = info.head;

        let mut cache = PageCache::with_capacity(1);
        let mut buffer = [0_u8; 4];
        cache
            .read(&fs, 0, document, revision, info.size, 0, &mut buffer)
            .expect("读第一页");
        cache
            .read(
                &fs,
                0,
                document,
                revision,
                info.size,
                PAGE_SIZE as u64,
                &mut buffer,
            )
            .expect("读第二页");
        assert_eq!(cache.len(), 1);
        cache
            .read(&fs, 0, document, revision, info.size, 0, &mut buffer)
            .expect("再读第一页");
        assert_eq!(cache.len(), 1);
        assert_eq!(&buffer, &[1; 4]);
    }
}
