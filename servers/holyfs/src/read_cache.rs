//! 压缩文档的解压缓存。
//!
//! 压缩文档按块读取时，文件系统会为每一块整篇解压。这里把压缩载荷读出来，
//! 交给工作线程一次性解压到缓存帧，后续块直接从缓存拷贝。缓存按
//! (盘符, 文档, head 修订) 校验，文档产生新修订后自动失效。

use hk_fs_holyfs::{DocId, DocumentKind, HolyError, HolyFs};
use hk_server_api::{map_new_lazy, yield_now};
use hk_worker::Pool;

use crate::volumes::ServiceDisk;

/// 单篇缓存上限。
const MAX_CACHE: usize = 2 * 1024 * 1024;

/// 单篇解压缓存。
pub(crate) struct ReadCache {
    pool: Option<&'static Pool>,
    drive: u8,
    document: u128,
    head: u64,
    virt: u64,
    length: usize,
    valid: bool,
}

impl ReadCache {
    /// 创建缓存；没有工作线程池时退化为不缓存。
    pub(crate) const fn new(pool: Option<&'static Pool>) -> Self {
        Self {
            pool,
            drive: 0,
            document: 0,
            head: 0,
            virt: 0,
            length: 0,
            valid: false,
        }
    }

    /// 读取文档的一段；命中或整篇解压成功后返回长度，未命中返回 `None`。
    pub(crate) fn read(
        &mut self,
        drive: &mut HolyFs<ServiceDisk>,
        letter: u8,
        document: DocId,
        revision: Option<u64>,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, HolyError> {
        let Some(pool) = self.pool else {
            return Ok(None);
        };
        let info = drive.document_info(document)?;
        if info.kind != DocumentKind::Content {
            return Ok(None);
        }
        let head = info.head.get();
        if revision.is_some_and(|revision| revision != head) {
            return Ok(None);
        }
        if self.matches(letter, document, head) {
            return Ok(Some(self.copy(offset, buffer)));
        }
        let (compressed, size) = drive.content_layout(document)?;
        if !compressed || size == 0 || size > MAX_CACHE {
            return Ok(None);
        }
        if !self.reserve() {
            return Ok(None);
        }
        let stored = drive.stored_content(document)?;
        let virt = self.virt;
        let payload = stored.payload;
        let _ = pool.submit(0, move || {
            // SAFETY: 缓存帧已映射，长度不小于 size；同一时刻只有一个解压任务。
            let output = unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, size) };
            u64::from(hk_fs_holyfs::decompress_into(&payload, output).is_err())
        });
        let failed = loop {
            if let Some((_, status)) = pool.try_next() {
                break status != 0;
            }
            yield_now();
        };
        if failed {
            return Ok(None);
        }
        self.drive = letter;
        self.document = document.get();
        self.head = head;
        self.length = size;
        self.valid = true;
        Ok(Some(self.copy(offset, buffer)))
    }

    fn matches(&self, letter: u8, document: DocId, head: u64) -> bool {
        self.valid
            && self.drive == letter
            && self.document == document.get()
            && self.head == head
            && self.length > 0
    }

    fn copy(&self, offset: u64, buffer: &mut [u8]) -> usize {
        let start = offset as usize;
        if start >= self.length {
            return 0;
        }
        let length = buffer.len().min(self.length - start);
        // SAFETY: 缓存帧已映射，长度为 self.length。
        let cached = unsafe { core::slice::from_raw_parts(self.virt as *const u8, self.length) };
        buffer[..length].copy_from_slice(&cached[start..start + length]);
        length
    }

    fn reserve(&mut self) -> bool {
        if self.virt != 0 {
            return true;
        }
        let Some((_handle, _region, virt)) = map_new_lazy(MAX_CACHE) else {
            return false;
        };
        self.virt = virt;
        true
    }
}
