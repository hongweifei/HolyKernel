//! 内核页表操作。
//!
//! 沿用 Limine 建立的页表，按需为 MMIO 建立 4 KiB 映射：缺失的中间级页表从物理
//! 帧分配器取页并清零，页表本身经 HHDM 访问。

use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::PagePool;

pub use hk_mem::LinearPages as PageAllocator;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
const COW: u64 = 1 << 9;
const NX: u64 = 1 << 63;
const FLAGS_MASK: u64 = 0xFFF | NX;
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const ENTRIES: usize = 512;
const KERNEL_PML4_START: usize = 256;

static DIRECT_MAP: AtomicU64 = AtomicU64::new(0);
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);
static COW_PAGES: PagePool = PagePool::new();
static USER_TABLE_PAGES: PagePool = PagePool::new();
static RESTORE_PAGES: PagePool = PagePool::new();

/// 设置页表运行时使用的 direct map 基址，并记录内核页表根。
pub fn init_runtime(direct_map: u64) {
    KERNEL_ROOT.store(read_cr3().get(), Ordering::Release);
    DIRECT_MAP.store(direct_map, Ordering::Release);
}

/// 返回 direct map 基址；未初始化时为 0。
pub fn direct_map_base() -> u64 {
    DIRECT_MAP.load(Ordering::Acquire)
}

/// 返回启动时记录的内核页表句柄；与当前 CR3 无关。
pub fn kernel_table() -> PageTable {
    PageTable {
        direct_map: DIRECT_MAP.load(Ordering::Acquire),
        root: PhysAddr::new(KERNEL_ROOT.load(Ordering::Acquire)),
    }
}

/// 预留写时复制与页表使用的物理帧。
pub fn reserve_cow_pages(allocator: &mut PageAllocator, count: usize) {
    COW_PAGES.reserve(allocator, count);
}

fn take_cow_page() -> Option<PhysAddr> {
    COW_PAGES.take()
}

/// 预留检查点恢复使用的物理帧。
pub fn reserve_restore_pages(allocator: &mut PageAllocator, count: usize) {
    RESTORE_PAGES.reserve(allocator, count);
}

/// 取出一个检查点恢复帧。
pub fn take_restore_page() -> Option<PhysAddr> {
    RESTORE_PAGES.take()
}

/// 归还一个检查点恢复帧。
pub fn give_restore_page(frame: PhysAddr) {
    RESTORE_PAGES.give(frame);
}

/// 预留 MMIO 映射所需的页表帧。
pub fn reserve_user_table_pages(allocator: &mut PageAllocator, count: usize) {
    USER_TABLE_PAGES.reserve(allocator, count);
}

fn take_user_table_frame() -> Option<PhysAddr> {
    USER_TABLE_PAGES.take()
}

/// 解析用户写保护页错误；成功复制页帧时返回 `true`。
pub fn resolve_cow(address: u64) -> bool {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 {
        return false;
    }
    let table = PageTable {
        direct_map,
        root: read_cr3(),
    };
    table.resolve_cow(address)
}

/// 判断当前页表中地址是否为用户可访问页。
pub fn is_user_mapped(address: u64) -> bool {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 {
        return false;
    }
    let table = PageTable {
        direct_map,
        root: read_cr3(),
    };
    table.is_user_mapped(address)
}

/// 页表项属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// 内核可读写页。
    pub const KERNEL_RW: Self = Self(WRITABLE);
    /// 用户可读写页。
    pub const USER_RW: Self = Self(WRITABLE | USER);
    /// 设备 MMIO 页：可写、写直达、禁止缓存。
    pub const MMIO: Self = Self(WRITABLE | PWT | PCD);
    /// 用户可访问的设备 MMIO 页。
    pub const USER_MMIO: Self = Self(WRITABLE | USER | PWT | PCD);

    const fn bits(self) -> u64 {
        self.0
    }

    /// 返回原始页表属性位。
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 从原始页表属性位构造。
    pub const fn from_raw(bits: u64) -> Self {
        Self(bits)
    }
}

/// 把快照属性转换为恢复用属性：清除 COW 与大页位，COW 页恢复为可写私有页。
pub const fn restore_flags(bits: u64) -> PageFlags {
    let writable = if bits & COW != 0 {
        WRITABLE
    } else {
        bits & WRITABLE
    };
    PageFlags((bits & !COW & !HUGE) | writable | USER)
}

fn push_unique(frames: &mut Vec<PhysAddr>, frame: PhysAddr) {
    if !frames.contains(&frame) {
        frames.push(frame);
    }
}

/// 页表操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingError {
    /// 没有可用的物理内存区域。
    NoUsableRegion,
    /// 物理帧耗尽。
    OutOfFrames,
    /// 地址未按页对齐。
    Unaligned,
    /// 命中了更大的映射，需要先拆分。
    HugePageConflict {
        /// 页表级别。
        level: u32,
        /// 冲突的虚拟地址。
        address: u64,
    },
}

/// 页表操作句柄。
pub struct PageTable {
    direct_map: u64,
    root: PhysAddr,
}

impl PageTable {
    /// 读取 CR3 建立句柄。
    pub fn new(direct_map: u64) -> Self {
        Self {
            direct_map,
            root: read_cr3(),
        }
    }

    /// 返回页表根物理地址。
    pub fn root(&self) -> PhysAddr {
        self.root
    }

    /// 返回同一页表根的共享句柄；用于同地址空间的新线程。
    pub fn share(&self) -> Self {
        Self {
            direct_map: self.direct_map,
            root: self.root,
        }
    }

    /// 以当前页表为模板创建用户页表：复制内核高半区映射，低半区留空。
    pub fn create_user(&self, allocator: &mut PageAllocator) -> Result<Self, PagingError> {
        self.create_user_with(|| allocator.allocate())
    }

    /// 以指定分配器创建用户页表；中间页表也由 `allocate` 提供。
    pub fn create_user_with<F>(&self, mut allocate: F) -> Result<Self, PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        let root = allocate().ok_or(PagingError::OutOfFrames)?;
        self.zero_page(root);

        for index in KERNEL_PML4_START..ENTRIES {
            // SAFETY: 两侧 PML4 均经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            // SAFETY: 目标 PML4 已清零且经 HHDM 可写，索引在 0..512 范围内。
            unsafe {
                self.entry(root, index).write_volatile(value);
            }
        }

        Ok(Self {
            direct_map: self.direct_map,
            root,
        })
    }

    /// 遍历低半区的 4 KiB 用户页；设备（PWT/PCD）页与大页跳过。
    pub fn user_pages<F>(&self, mut visit: F)
    where
        F: FnMut(u64, u64, u64),
    {
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 || pml4e & USER == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 || pdp & USER == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & USER == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & USER == 0 {
                            continue;
                        }
                        if pte & (PWT | PCD) != 0 {
                            continue;
                        }
                        let virt = ((pml4 as u64) << 39)
                            | ((pdpt_index as u64) << 30)
                            | ((pd_index as u64) << 21)
                            | ((pt_index as u64) << 12);
                        visit(virt, pte & FLAGS_MASK, pte & ADDRESS_MASK);
                    }
                }
            }
        }
    }

    /// 判断低半区是否存在写时复制页；此类任务暂不支持检查点。
    pub fn has_cow_user_page(&self) -> bool {
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT != 0 && pte & COW != 0 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// 收集低半区用户页表与数据帧，供检查点释放或复用。
    pub fn user_frames(&self) -> Vec<PhysAddr> {
        let mut frames = Vec::new();
        push_unique(&mut frames, self.root);
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            push_unique(&mut frames, pdpt);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                push_unique(&mut frames, pd);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    push_unique(&mut frames, pt);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & (PWT | PCD) != 0 {
                            continue;
                        }
                        push_unique(&mut frames, PhysAddr::new(pte & ADDRESS_MASK));
                    }
                }
            }
        }
        frames
    }

    /// 从物理帧读取一段字节；帧经 direct map 访问。
    pub fn read_phys(&self, phys: PhysAddr, out: &mut [u8]) {
        let source = (self.direct_map + phys.get()) as *const u8;
        // SAFETY: 物理帧位于 direct map 内，调用方保证长度不超过一页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, out.as_mut_ptr(), out.len());
        }
    }

    /// 向物理帧写入一段字节；帧经 direct map 访问。
    pub fn write_phys(&self, phys: PhysAddr, data: &[u8]) {
        let target = (self.direct_map + phys.get()) as *mut u8;
        // SAFETY: 物理帧位于 direct map 内，调用方保证长度不超过一页。
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
        }
    }

    /// 清空一个物理帧。
    pub fn zero_phys(&self, phys: PhysAddr) {
        let target = (self.direct_map + phys.get()) as *mut u8;
        // SAFETY: 物理帧位于 direct map 内，长度为整页。
        unsafe {
            core::ptr::write_bytes(target, 0, PAGE_SIZE);
        }
    }

    /// 切换到该页表。
    pub fn activate(&self) {
        // SAFETY: 根物理地址来自页表或帧分配器；写入 CR3 只影响当前 CPU。
        unsafe {
            asm!("mov cr3, {}", in(reg) self.root.get(), options(nostack, preserves_flags));
        }
    }

    /// 复制用户低半区并标记写时复制，返回子页表。
    pub fn fork_user(&self) -> Option<Self> {
        let child_root = take_cow_page()?;
        self.zero_page(child_root);

        for index in KERNEL_PML4_START..ENTRIES {
            // SAFETY: 两侧 PML4 均经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            // SAFETY: 目标 PML4 已清零且经 HHDM 可写，索引在 0..512 范围内。
            unsafe {
                self.entry(child_root, index).write_volatile(value);
            }
        }

        for index in 0..KERNEL_PML4_START {
            // SAFETY: 读取模板 PML4 项，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            if value & PRESENT == 0 {
                continue;
            }
            if value & HUGE != 0 {
                return None;
            }

            let child = take_cow_page()?;
            self.zero_page(child);
            // SAFETY: 父页表处于活动状态，条目地址经 HHDM 映射。
            let source = PhysAddr::new(value & ADDRESS_MASK);
            self.clone_level(source, child, 3)?;
            // SAFETY: 子 PML4 项指向新分配的 PDPT，并保留原有权限位。
            unsafe {
                self.entry(child_root, index)
                    .write_volatile(child.get() | (value & !ADDRESS_MASK));
            }
        }

        if !crate::cpu::shootdown_tlb(None) {
            return None;
        }
        self.activate();
        Some(Self {
            direct_map: self.direct_map,
            root: child_root,
        })
    }

    fn clone_level(&self, source: PhysAddr, target: PhysAddr, level: u32) -> Option<()> {
        for index in 0..ENTRIES {
            // SAFETY: 源页表经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(source, index).read_volatile() };
            if value & PRESENT == 0 {
                continue;
            }
            if value & HUGE != 0 {
                return None;
            }

            let physical = value & ADDRESS_MASK;
            let flags = value & !ADDRESS_MASK;
            if level == 1 {
                let mut leaf = flags;
                if leaf & WRITABLE != 0 {
                    leaf = (leaf & !WRITABLE) | COW;
                    // SAFETY: 父页表叶项经 HHDM 可写，保持不变共享页帧。
                    unsafe {
                        self.entry(source, index).write_volatile(physical | leaf);
                    }
                }
                // SAFETY: 子页表叶项指向共享页帧，并带有 COW 标记。
                unsafe {
                    self.entry(target, index).write_volatile(physical | leaf);
                }
            } else {
                let child = take_cow_page()?;
                self.zero_page(child);
                self.clone_level(PhysAddr::new(physical), child, level - 1)?;
                // SAFETY: 子中间级项指向新页表，并保留原有权限位。
                unsafe {
                    self.entry(target, index)
                        .write_volatile(child.get() | flags);
                }
            }
        }
        Some(())
    }

    /// 处理写时复制页错误：复制页帧并恢复可写映射。
    pub fn resolve_cow(&self, address: u64) -> bool {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 || value & HUGE != 0 {
                return false;
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }

        let index = ((address >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        if value & PRESENT == 0 || value & COW == 0 || value & WRITABLE != 0 {
            return false;
        }

        let Some(frame) = take_cow_page() else {
            return false;
        };
        let source = (self.direct_map + (value & ADDRESS_MASK)) as *const u8;
        let target = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 源页帧与目标页帧均经 HHDM 映射，长度为整页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, target, PAGE_SIZE);
        }

        let updated = frame.get() | (value & !ADDRESS_MASK & !COW) | WRITABLE;
        // SAFETY: 末级页表项经 HHDM 可写，索引在 0..512 范围内。
        unsafe {
            self.entry(table, index).write_volatile(updated);
            asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
        }
        true
    }

    /// 判断地址是否映射为用户可访问页；大页直接视为已映射。
    pub fn is_user_mapped(&self, address: u64) -> bool {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 || value & USER == 0 {
                return false;
            }
            if value & HUGE != 0 {
                return true;
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }
        let index = ((address >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        value & PRESENT != 0 && value & USER != 0
    }

    /// 建立 4 KiB 映射。
    pub fn map(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        allocator: &mut PageAllocator,
    ) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, || allocator.allocate())
    }

    /// 建立映射，并用调用方提供的分配器分配中间页表。
    pub fn map_with<F>(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        mut allocate: F,
    ) -> Result<(), PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        if !virt.is_page_aligned() || !phys.is_page_aligned() {
            return Err(PagingError::Unaligned);
        }

        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((virt.get() >> level) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { entry.read_volatile() };

            if value & PRESENT == 0 {
                let frame = allocate().ok_or(PagingError::OutOfFrames)?;
                self.zero_page(frame);
                // SAFETY: 同上；新页表已清零，写入指向它的当前级表项。
                // 中间级必须带 USER，否则 ring 3 访问会被拒绝。
                unsafe {
                    entry.write_volatile(frame.get() | PRESENT | WRITABLE | USER);
                }
                table = frame;
            } else if value & HUGE != 0 {
                return Err(PagingError::HugePageConflict {
                    level,
                    address: virt.get(),
                });
            } else {
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
        }

        let index = ((virt.get() >> 12) & 0x1FF) as usize;
        let entry = self.entry(table, index);
        // SAFETY: 末级页表项同样位于 HHDM 映射的物理内存。
        unsafe {
            entry.write_volatile(phys.get() | flags.bits() | PRESENT);
        }
        // SAFETY: 刷新已修改的页对应的一级 TLB 项。
        unsafe {
            asm!("invlpg [{}]", in(reg) virt.get(), options(nostack, preserves_flags));
        }
        Ok(())
    }

    /// 解除一段用户映射；页表中间级保留。
    pub fn unmap_user_range(&self, virt: VirtAddr, len: usize) -> bool {
        if !virt.is_page_aligned() {
            return false;
        }
        let mut offset = 0;
        while offset < len {
            let address = virt.get() + offset as u64;
            let mut table = self.root;
            let mut present = true;
            for level in [39_u32, 30, 21] {
                let index = ((address >> level) & 0x1FF) as usize;
                let entry = self.entry(table, index);
                // SAFETY: 页表项位于 HHDM 映射的物理内存。
                let value = unsafe { entry.read_volatile() };
                if value & PRESENT == 0 || value & HUGE != 0 {
                    present = false;
                    break;
                }
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
            if !present {
                return false;
            }
            let index = ((address >> 12) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 末级页表项位于 HHDM 映射的物理内存。
            unsafe {
                entry.write_volatile(0);
                asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
            }
            offset += PAGE_SIZE;
        }
        true
    }

    /// 使用 MMIO 页表帧池建立映射。
    pub fn map_mmio(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
    ) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, take_user_table_frame)
    }

    fn entry(&self, table: PhysAddr, index: usize) -> *mut u64 {
        debug_assert!(index < ENTRIES);
        ((self.direct_map + table.get()) as *mut u64).wrapping_add(index)
    }

    fn zero_page(&self, frame: PhysAddr) {
        let pointer = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 新分配的帧来自可用内存区域，经 HHDM 可写。
        unsafe {
            core::ptr::write_bytes(pointer, 0, PAGE_SIZE);
        }
    }
}

fn read_cr3() -> PhysAddr {
    let value: u64;
    // SAFETY: 读取 CR3 不改变机器状态；只在 ring 0 执行。
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack));
    }
    PhysAddr::new(value & ADDRESS_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_user_table_with_kernel_entries() {
        let mut memory = alloc::vec![0_u8; 3 * PAGE_SIZE];
        let base = memory.as_mut_ptr() as u64;
        // SAFETY: 缓冲区覆盖模板与用户页表各一页。
        unsafe {
            let root = base as *mut u64;
            root.add(0).write_volatile(0x9ABC);
            root.add(256).write_volatile(0x1234);
            root.add(511).write_volatile(0x5678);
        }

        let table = PageTable {
            direct_map: base,
            root: PhysAddr::new(0),
        };
        let mut allocator = PageAllocator::from_range(PAGE_SIZE as u64, 2 * PAGE_SIZE as u64);
        let user = table.create_user(&mut allocator).expect("创建用户页表");
        assert_eq!(user.root, PhysAddr::new(PAGE_SIZE as u64));

        // SAFETY: 用户页表位于缓冲区第二页，已清零并复制内核入口。
        unsafe {
            let root = (base + PAGE_SIZE as u64) as *const u64;
            assert_eq!(root.add(0).read_volatile(), 0);
            assert_eq!(root.add(256).read_volatile(), 0x1234);
            assert_eq!(root.add(511).read_volatile(), 0x5678);
        }
    }
}
