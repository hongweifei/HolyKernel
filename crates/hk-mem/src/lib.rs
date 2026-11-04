//! 内存对象：Untyped、Frame、VSpace 与写时复制。
//!
//! 内核只提供机制：Untyped 切分物理内存，Frame 表达物理页，VSpace 记录映射并
//! 驱动平台 MMU。写时复制由只读映射、`cow` 标记与页错误处理三步完成。

#![no_std]

extern crate alloc;

pub mod dma;
pub mod frame_pool;
pub mod frames;
pub mod iommu;
pub mod vspace_registry;

pub use dma::DmaPool;
pub use frame_pool::FramePool;
pub use frames::LinearFrames;
pub use iommu::SoftwareIommu;
pub use vspace_registry::{UserVspace, VspaceRegistry};

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use hk_hal::{
    MapFlags, Mmu, MmuError, PAGE_SHIFT, PAGE_SIZE, PhysAddr, PhysRange, PhysicalAccess,
    PhysicalError, VSpaceId, VirtAddr,
};
use hk_handle::{Object, ObjectHeader, ObjectKind};
use spin::Mutex;

/// 内存对象操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemError {
    /// 物理内存不足。
    OutOfMemory,
    /// 地址或长度未按页对齐。
    Unaligned,
    /// 映射同时要求可写与可执行。
    WxConflict,
    /// 目标虚拟地址已有映射。
    AlreadyMapped,
    /// 目标虚拟地址没有映射。
    NotMapped,
    /// 页不是写时复制页，写错误不可由内核修复。
    NotCowPage,
    /// Untyped 区域为空。
    EmptyRegion,
    /// 平台 MMU 报错。
    Mmu(MmuError),
    /// 平台物理内存访问报错。
    Physical(PhysicalError),
}

impl fmt::Display for MemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => formatter.write_str("物理内存不足"),
            Self::Unaligned => formatter.write_str("地址或长度未按页对齐"),
            Self::WxConflict => formatter.write_str("映射同时要求可写与可执行"),
            Self::AlreadyMapped => formatter.write_str("虚拟地址已有映射"),
            Self::NotMapped => formatter.write_str("虚拟地址没有映射"),
            Self::NotCowPage => formatter.write_str("页不是写时复制页"),
            Self::EmptyRegion => formatter.write_str("Untyped 区域为空"),
            Self::Mmu(error) => write!(formatter, "MMU 错误：{error}"),
            Self::Physical(error) => write!(formatter, "物理内存访问错误：{error}"),
        }
    }
}

impl core::error::Error for MemError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Mmu(error) => Some(error),
            Self::Physical(error) => Some(error),
            _ => None,
        }
    }
}

impl From<MmuError> for MemError {
    fn from(error: MmuError) -> Self {
        match error {
            MmuError::WxConflict => Self::WxConflict,
            MmuError::Unaligned => Self::Unaligned,
            MmuError::AlreadyMapped => Self::AlreadyMapped,
            MmuError::NotMapped => Self::NotMapped,
            other => Self::Mmu(other),
        }
    }
}

impl From<PhysicalError> for MemError {
    fn from(error: PhysicalError) -> Self {
        Self::Physical(error)
    }
}

/// 物理页帧。
pub struct Frame {
    header: Arc<ObjectHeader>,
    phys: PhysAddr,
    len: usize,
}

/// 最后一个 Frame 句柄释放时归还物理区间的回调。
static FRAME_RELEASER: AtomicUsize = AtomicUsize::new(0);

/// 注册 Frame 物理区间回收回调；重复调用会替换旧回调。
pub fn set_frame_releaser(release: fn(PhysAddr, usize)) {
    FRAME_RELEASER.store(release as usize, Ordering::Release);
}

impl Frame {
    /// 创建页帧对象并分配对象头。
    pub fn new(phys: PhysAddr, len: usize) -> Self {
        Self {
            header: ObjectHeader::new(ObjectKind::Frame),
            phys,
            len,
        }
    }

    /// 返回页帧起始物理地址。
    pub fn phys(&self) -> PhysAddr {
        self.phys
    }

    /// 返回页帧长度。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 判断页帧长度是否为 0。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回对象头的共享引用。
    pub fn header_arc(&self) -> &Arc<ObjectHeader> {
        &self.header
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let release = FRAME_RELEASER.load(Ordering::Acquire);
        if release != 0 {
            // SAFETY: 回调由平台在启动期注册，签名与存储的地址一致。
            let release: fn(PhysAddr, usize) = unsafe { core::mem::transmute(release) };
            release(self.phys, self.len);
        }
    }
}

impl Object for Frame {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
}

/// 未类型化物理内存。
pub struct Untyped {
    header: Arc<ObjectHeader>,
    region: PhysRange,
    watermark: PhysAddr,
}

impl Untyped {
    /// 以物理区域创建 Untyped。
    pub fn new(region: PhysRange) -> Self {
        Self {
            header: ObjectHeader::new(ObjectKind::Untyped),
            watermark: region.start(),
            region,
        }
    }

    /// 返回可分配区域。
    pub fn region(&self) -> PhysRange {
        self.region
    }

    /// 返回剩余字节数。
    pub fn remaining(&self) -> u64 {
        self.region.end().get().saturating_sub(self.watermark.get())
    }

    /// 切分一个页帧。
    pub fn retype_frame(&mut self, len: usize) -> Result<Frame, MemError> {
        let phys = self.take(len)?;
        Ok(Frame::new(phys, len))
    }

    /// 切分一段子 Untyped。
    pub fn retype_untyped(&mut self, len: usize) -> Result<Untyped, MemError> {
        let phys = self.take(len)?;
        Ok(Untyped::new(PhysRange::new(phys, len as u64)))
    }

    fn take(&mut self, len: usize) -> Result<PhysAddr, MemError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) {
            return Err(MemError::Unaligned);
        }
        let start = self.watermark.align_up();
        let end = start.get() + len as u64;
        if end > self.region.end().get() {
            return Err(MemError::OutOfMemory);
        }
        self.watermark = PhysAddr::new(end);
        Ok(start)
    }
}

impl Object for Untyped {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
}

/// 页帧分配来源。
pub trait FrameAllocator {
    /// 分配长度为 `len` 的页帧。
    fn allocate(&self, len: usize) -> Result<Frame, MemError>;
}

#[derive(Clone)]
struct PageRecord {
    phys: PhysAddr,
    flags: MapFlags,
    cow: bool,
    frame: Arc<ObjectHeader>,
}

/// 进程地址空间。
pub struct VSpace {
    header: Arc<ObjectHeader>,
    id: VSpaceId,
    pages: Mutex<BTreeMap<u64, PageRecord>>,
}

impl VSpace {
    /// 以平台分配的地址空间标识创建 VSpace。
    pub fn new(id: VSpaceId) -> Self {
        Self {
            header: ObjectHeader::new(ObjectKind::VSpace),
            id,
            pages: Mutex::new(BTreeMap::new()),
        }
    }

    /// 返回地址空间标识。
    pub fn id(&self) -> VSpaceId {
        self.id
    }

    /// 返回已映射页数。
    pub fn mapped_pages(&self) -> usize {
        self.pages.lock().len()
    }

    /// 判断虚拟地址所在页是否标记写时复制。
    pub fn is_cow(&self, virt: VirtAddr) -> bool {
        self.pages
            .lock()
            .get(&virt.page_index())
            .is_some_and(|record| record.cow)
    }

    /// 建立映射。
    pub fn map(
        &self,
        mmu: &dyn Mmu,
        frame: &Frame,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MemError> {
        if len == 0
            || !len.is_multiple_of(PAGE_SIZE)
            || !virt.is_page_aligned()
            || !frame.phys().is_page_aligned()
        {
            return Err(MemError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MemError::WxConflict);
        }

        let pages = (len / PAGE_SIZE) as u64;
        {
            let records = self.pages.lock();
            for offset in 0..pages {
                if records.contains_key(&(virt.page_index() + offset)) {
                    return Err(MemError::AlreadyMapped);
                }
            }
        }

        mmu.map(self.id, frame.phys(), virt, len, flags)?;

        let mut records = self.pages.lock();
        for offset in 0..pages {
            records.insert(
                virt.page_index() + offset,
                PageRecord {
                    phys: PhysAddr::new(frame.phys().get() + offset * PAGE_SIZE as u64),
                    flags,
                    cow: false,
                    frame: Arc::clone(frame.header_arc()),
                },
            );
        }
        Ok(())
    }

    /// 解除映射。
    pub fn unmap(&self, mmu: &dyn Mmu, virt: VirtAddr, len: usize) -> Result<(), MemError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MemError::Unaligned);
        }

        let pages = (len / PAGE_SIZE) as u64;
        {
            let records = self.pages.lock();
            for offset in 0..pages {
                if !records.contains_key(&(virt.page_index() + offset)) {
                    return Err(MemError::NotMapped);
                }
            }
        }

        mmu.unmap(self.id, virt, len)?;

        let mut records = self.pages.lock();
        for offset in 0..pages {
            records.remove(&(virt.page_index() + offset));
        }
        Ok(())
    }

    /// 修改映射属性。
    pub fn protect(
        &self,
        mmu: &dyn Mmu,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MemError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MemError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MemError::WxConflict);
        }

        let pages = (len / PAGE_SIZE) as u64;
        {
            let records = self.pages.lock();
            for offset in 0..pages {
                if !records.contains_key(&(virt.page_index() + offset)) {
                    return Err(MemError::NotMapped);
                }
            }
        }

        mmu.protect(self.id, virt, len, flags)?;

        let mut records = self.pages.lock();
        for offset in 0..pages {
            if let Some(record) = records.get_mut(&(virt.page_index() + offset)) {
                record.flags = flags;
            }
        }
        Ok(())
    }

    /// 查询虚拟地址的物理地址与属性。
    pub fn translate(&self, mmu: &dyn Mmu, virt: VirtAddr) -> Option<(PhysAddr, MapFlags)> {
        mmu.translate(self.id, virt)
    }

    /// 把一段映射标记为写时复制：权限收窄为只读。
    pub fn mark_cow(&self, mmu: &dyn Mmu, virt: VirtAddr, len: usize) -> Result<(), MemError> {
        let read_only = MapFlags::READ;
        self.protect(mmu, virt, len, read_only)?;

        let pages = (len / PAGE_SIZE) as u64;
        let mut records = self.pages.lock();
        for offset in 0..pages {
            if let Some(record) = records.get_mut(&(virt.page_index() + offset)) {
                record.cow = true;
            }
        }
        Ok(())
    }

    /// 把全部映射共享到子地址空间，并把可写页转为写时复制。
    pub fn fork_into(&self, mmu: &dyn Mmu, child: &VSpace) -> Result<(), MemError> {
        let snapshot: BTreeMap<u64, PageRecord> = self.pages.lock().clone();

        for (page, record) in &snapshot {
            if child.pages.lock().contains_key(page) {
                return Err(MemError::AlreadyMapped);
            }

            let child_flags = if record.flags.contains(MapFlags::WRITE) {
                record.flags - MapFlags::WRITE
            } else {
                record.flags
            };
            let virt = VirtAddr::new(page << PAGE_SHIFT);
            mmu.map(child.id, record.phys, virt, PAGE_SIZE, child_flags)?;
            child.pages.lock().insert(
                *page,
                PageRecord {
                    phys: record.phys,
                    flags: child_flags,
                    cow: true,
                    frame: Arc::clone(&record.frame),
                },
            );

            if record.flags.contains(MapFlags::WRITE) {
                mmu.protect(self.id, virt, PAGE_SIZE, child_flags)?;
                let mut records = self.pages.lock();
                if let Some(record) = records.get_mut(page) {
                    record.flags = child_flags;
                    record.cow = true;
                }
            }
        }
        Ok(())
    }

    /// 处理写时复制页错误：复制页帧并换上新映射。
    pub fn resolve_write_fault(
        &self,
        mmu: &dyn Mmu,
        physical: &dyn PhysicalAccess,
        allocator: &dyn FrameAllocator,
        virt: VirtAddr,
    ) -> Result<PhysAddr, MemError> {
        let page = virt.page_index();
        let record = self
            .pages
            .lock()
            .get(&page)
            .cloned()
            .ok_or(MemError::NotMapped)?;
        if !record.cow {
            return Err(MemError::NotCowPage);
        }

        let frame = allocator.allocate(PAGE_SIZE)?;
        physical.copy(record.phys, frame.phys(), PAGE_SIZE)?;

        let page_virt = VirtAddr::new(page << PAGE_SHIFT);
        let flags = record.flags | MapFlags::WRITE;
        mmu.unmap(self.id, page_virt, PAGE_SIZE)?;
        mmu.map(self.id, frame.phys(), page_virt, PAGE_SIZE, flags)?;

        let mut records = self.pages.lock();
        records.insert(
            page,
            PageRecord {
                phys: frame.phys(),
                flags,
                cow: false,
                frame: Arc::clone(frame.header_arc()),
            },
        );
        Ok(frame.phys())
    }
}

impl Object for VSpace {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
}

impl fmt::Debug for VSpace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VSpace")
            .field("id", &self.id)
            .field("pages", &self.mapped_pages())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::PhysicalAccess;
    use hk_platform_hosted::mmu::HostedMmu;
    use hk_platform_hosted::physical::HostedMemory;

    struct TestAllocator {
        inner: Mutex<Untyped>,
    }

    impl TestAllocator {
        fn new(base: u64, len: u64) -> Self {
            Self {
                inner: Mutex::new(Untyped::new(PhysRange::new(PhysAddr::new(base), len))),
            }
        }
    }

    impl FrameAllocator for TestAllocator {
        fn allocate(&self, len: usize) -> Result<Frame, MemError> {
            self.inner.lock().retype_frame(len)
        }
    }

    struct Fixture {
        mmu: HostedMmu,
        memory: HostedMemory,
        allocator: TestAllocator,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                mmu: HostedMmu::new(),
                memory: HostedMemory::new(PhysRange::new(PhysAddr::new(0x200_0000), 0x100_000)),
                allocator: TestAllocator::new(0x100_0000, 0x100_000),
            }
        }

        fn vspace(&self) -> VSpace {
            VSpace::new(self.mmu.create_vspace())
        }

        fn frame(&self) -> Frame {
            self.allocator.allocate(PAGE_SIZE).expect("分配页帧")
        }
    }

    #[test]
    fn untyped_splits_frames_until_exhausted() {
        let mut untyped = Untyped::new(PhysRange::new(
            PhysAddr::new(0x10_0000),
            3 * PAGE_SIZE as u64,
        ));
        assert_eq!(untyped.remaining(), 3 * PAGE_SIZE as u64);

        let first = untyped.retype_frame(PAGE_SIZE).expect("第一个页帧");
        let second = untyped.retype_frame(PAGE_SIZE).expect("第二个页帧");
        assert_eq!(first.phys(), PhysAddr::new(0x10_0000));
        assert_eq!(second.phys(), PhysAddr::new(0x10_1000));
        assert_eq!(untyped.remaining(), PAGE_SIZE as u64);

        let extra = untyped.retype_frame(2 * PAGE_SIZE);
        assert_eq!(extra.err(), Some(MemError::OutOfMemory));
    }

    #[test]
    fn untyped_rejects_unaligned_slices() {
        let mut untyped = Untyped::new(PhysRange::new(PhysAddr::new(0x10_0000), 0x2000));
        assert_eq!(untyped.retype_frame(0x800).err(), Some(MemError::Unaligned));
    }

    #[test]
    fn maps_and_unmaps_pages() {
        let fixture = Fixture::new();
        let vspace = fixture.vspace();
        let frame = fixture.frame();
        let virt = VirtAddr::new(0x40_0000);

        vspace
            .map(
                &fixture.mmu,
                &frame,
                virt,
                PAGE_SIZE,
                MapFlags::READ | MapFlags::WRITE,
            )
            .expect("映射成功");
        assert_eq!(vspace.mapped_pages(), 1);
        assert_eq!(
            vspace.translate(&fixture.mmu, VirtAddr::new(0x40_0010)),
            Some((frame.phys().offset(0x10), MapFlags::READ | MapFlags::WRITE))
        );

        vspace
            .unmap(&fixture.mmu, virt, PAGE_SIZE)
            .expect("解除映射");
        assert_eq!(vspace.mapped_pages(), 0);
        assert_eq!(vspace.translate(&fixture.mmu, virt), None);
    }

    #[test]
    fn rejects_wx_and_overlapping_mappings() {
        let fixture = Fixture::new();
        let vspace = fixture.vspace();
        let frame = fixture.frame();
        let virt = VirtAddr::new(0x40_0000);

        assert_eq!(
            vspace
                .map(
                    &fixture.mmu,
                    &frame,
                    virt,
                    PAGE_SIZE,
                    MapFlags::READ | MapFlags::WRITE | MapFlags::EXECUTE
                )
                .err(),
            Some(MemError::WxConflict)
        );

        vspace
            .map(&fixture.mmu, &frame, virt, PAGE_SIZE, MapFlags::READ)
            .expect("映射成功");
        assert_eq!(
            vspace
                .map(&fixture.mmu, &frame, virt, PAGE_SIZE, MapFlags::READ)
                .err(),
            Some(MemError::AlreadyMapped)
        );
    }

    #[test]
    fn fork_marks_pages_copy_on_write() {
        let fixture = Fixture::new();
        let parent = fixture.vspace();
        let child = fixture.vspace();
        let frame = fixture.frame();
        let virt = VirtAddr::new(0x40_0000);

        parent
            .map(
                &fixture.mmu,
                &frame,
                virt,
                PAGE_SIZE,
                MapFlags::READ | MapFlags::WRITE,
            )
            .expect("映射成功");
        parent
            .fork_into(&fixture.mmu, &child)
            .expect("复制地址空间");

        assert!(parent.is_cow(virt));
        assert!(child.is_cow(virt));
        assert_eq!(
            child.translate(&fixture.mmu, virt).map(|(_, flags)| flags),
            Some(MapFlags::READ)
        );

        let written = [0xAB_u8; 16];
        fixture
            .memory
            .write(frame.phys(), &written)
            .expect("写入页帧");
    }

    #[test]
    fn resolves_write_fault_with_private_copy() {
        let fixture = Fixture::new();
        let parent = fixture.vspace();
        let child = fixture.vspace();
        let frame = fixture.frame();
        let virt = VirtAddr::new(0x40_0000);

        parent
            .map(
                &fixture.mmu,
                &frame,
                virt,
                PAGE_SIZE,
                MapFlags::READ | MapFlags::WRITE,
            )
            .expect("映射成功");
        fixture
            .memory
            .write(frame.phys(), &[0x11; 32])
            .expect("写入初始内容");
        parent
            .fork_into(&fixture.mmu, &child)
            .expect("复制地址空间");

        let new_phys = parent
            .resolve_write_fault(&fixture.mmu, &fixture.memory, &fixture.allocator, virt)
            .expect("处理写错误");

        assert_ne!(new_phys, frame.phys());
        assert!(!parent.is_cow(virt));
        assert!(child.is_cow(virt));

        let mut parent_bytes = [0_u8; 32];
        fixture
            .memory
            .read(new_phys, &mut parent_bytes)
            .expect("读取父页");
        assert_eq!(parent_bytes, [0x11; 32]);

        let mut child_bytes = [0_u8; 32];
        fixture
            .memory
            .read(frame.phys(), &mut child_bytes)
            .expect("读取子页");
        assert_eq!(child_bytes, [0x11; 32]);

        fixture.memory.write(new_phys, &[0x22; 32]).expect("写父页");
        fixture
            .memory
            .read(frame.phys(), &mut child_bytes)
            .expect("复读子页");
        assert_eq!(child_bytes, [0x11; 32]);
    }

    #[test]
    fn rejects_write_fault_on_shared_page_after_copy() {
        let fixture = Fixture::new();
        let vspace = fixture.vspace();
        let frame = fixture.frame();
        let virt = VirtAddr::new(0x40_0000);

        vspace
            .map(
                &fixture.mmu,
                &frame,
                virt,
                PAGE_SIZE,
                MapFlags::READ | MapFlags::WRITE,
            )
            .expect("映射成功");
        assert_eq!(
            vspace
                .resolve_write_fault(&fixture.mmu, &fixture.memory, &fixture.allocator, virt)
                .err(),
            Some(MemError::NotCowPage)
        );
    }
}
