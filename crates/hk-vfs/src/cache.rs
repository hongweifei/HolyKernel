//! 文件页缓存：按 4 KiB 页缓存后端数据。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;

use crate::vfs::{Backend, NodeId, VfsError};

/// 页大小。
pub const PAGE_SIZE: usize = 4096;

type PageKey = (usize, NodeId, u64);
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

    /// 使某节点的全部页失效。
    pub fn invalidate(&mut self, instance: usize, node: NodeId) {
        self.pages.retain(|(cached_instance, cached_node, _), _| {
            *cached_instance != instance || *cached_node != node
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

    /// 从文件读取，未命中时从后端加载整页。
    pub fn read<B: Backend + ?Sized>(
        &mut self,
        backend: &B,
        instance: usize,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let size = backend.metadata(node)?.size;
        if offset >= size {
            return Ok(0);
        }
        let available = usize::try_from(size - offset).unwrap_or(usize::MAX);
        let limit = buffer.len().min(available);
        let mut done = 0;

        while done < limit {
            let position = offset + done as u64;
            let index = position / PAGE_SIZE as u64;
            let within = (position % PAGE_SIZE as u64) as usize;
            let page = self.load(backend, instance, node, index)?;
            let length = (PAGE_SIZE - within).min(limit - done);
            buffer[done..done + length].copy_from_slice(&page[within..within + length]);
            done += length;
        }
        Ok(done)
    }

    /// 写入文件；写穿透后端并更新缓存页。
    pub fn write<B: Backend + ?Sized>(
        &mut self,
        backend: &mut B,
        instance: usize,
        node: NodeId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut done = 0;
        while done < data.len() {
            let position = offset + done as u64;
            let index = position / PAGE_SIZE as u64;
            let within = (position % PAGE_SIZE as u64) as usize;
            let length = (PAGE_SIZE - within).min(data.len() - done);
            let page = self.load(backend, instance, node, index)?;
            page[within..within + length].copy_from_slice(&data[done..done + length]);
            backend.write(node, position, &page[within..within + length])?;
            done += length;
        }
        Ok(done)
    }

    fn load<B: Backend + ?Sized>(
        &mut self,
        backend: &B,
        instance: usize,
        node: NodeId,
        index: u64,
    ) -> Result<&mut Page, VfsError> {
        let key = (instance, node, index);
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
            backend.read(node, index * PAGE_SIZE as u64, &mut page[..])?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tmpfs;
    use crate::vfs::{FileType, OpenFlags, Vfs};

    #[test]
    fn caches_pages_across_reads() {
        let mut fs = Tmpfs::new();
        let file = fs.create(0, "file", FileType::File).expect("创建");
        fs.write(file, 0, b"hello").expect("写入");

        let mut cache = PageCache::new();
        let mut buffer = [0_u8; 5];
        assert_eq!(cache.read(&fs, 0, file, 0, &mut buffer).expect("读"), 5);
        assert_eq!(&buffer, b"hello");
        assert_eq!(cache.len(), 1);

        let mut tail = [0_u8; 3];
        assert_eq!(cache.read(&fs, 0, file, 2, &mut tail).expect("读"), 3);
        assert_eq!(&tail, b"llo");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn writes_through_and_updates_cache() {
        let mut fs = Tmpfs::new();
        let file = fs.create(0, "file", FileType::File).expect("创建");
        let mut cache = PageCache::new();
        cache.write(&mut fs, 0, file, 0, b"abc").expect("写");
        assert_eq!(fs.metadata(file).expect("元数据").size, 3);

        let mut buffer = [0_u8; 3];
        assert_eq!(cache.read(&fs, 0, file, 0, &mut buffer).expect("读"), 3);
        assert_eq!(&buffer, b"abc");
    }

    #[test]
    fn vfs_reads_use_cache() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
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
        let mut fs = Tmpfs::new();
        let file = fs.create(0, "file", FileType::File).expect("创建");
        let first = [1_u8; PAGE_SIZE];
        let second = [2_u8; PAGE_SIZE];
        fs.write(file, 0, &first).expect("写第一页");
        fs.write(file, PAGE_SIZE as u64, &second).expect("写第二页");

        let mut cache = PageCache::with_capacity(1);
        let mut buffer = [0_u8; 4];
        cache.read(&fs, 0, file, 0, &mut buffer).expect("读第一页");
        cache
            .read(&fs, 0, file, PAGE_SIZE as u64, &mut buffer)
            .expect("读第二页");
        assert_eq!(cache.len(), 1);
        cache
            .read(&fs, 0, file, 0, &mut buffer)
            .expect("再读第一页");
        assert_eq!(cache.len(), 1);
        assert_eq!(&buffer, &[1; 4]);
    }
}
