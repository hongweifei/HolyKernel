//! 内核页表操作。
//!
//! 沿用 Limine 建立的页表，按需为 MMIO 建立 4 KiB 映射：缺失的中间级页表从物理
//! 帧分配器取页并清零，页表本身经 HHDM 访问。

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::FramePool;

pub use hk_mem::LinearFrames as FrameAllocator;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
const COW: u64 = 1 << 9;
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const ENTRIES: usize = 512;
const KERNEL_PML4_START: usize = 256;

static DIRECT_MAP: AtomicU64 = AtomicU64::new(0);
static COW_FRAMES: FramePool = FramePool::new();
static USER_TABLE_FRAMES: FramePool = FramePool::new();

/// 设置页表运行时使用的 direct map 基址。
pub fn init_runtime(direct_map: u64) {
    DIRECT_MAP.store(direct_map, Ordering::Release);
}

/// 预留写时复制与页表使用的物理帧。
pub fn reserve_frames(allocator: &mut FrameAllocator, count: usize) {
    COW_FRAMES.reserve(allocator, count);
}

fn take_frame() -> Option<PhysAddr> {
    COW_FRAMES.take()
}

/// 预留 MMIO 映射所需的页表帧。
pub fn reserve_user_table_frames(allocator: &mut FrameAllocator, count: usize) {
    USER_TABLE_FRAMES.reserve(allocator, count);
}

fn take_user_table_frame() -> Option<PhysAddr> {
    USER_TABLE_FRAMES.take()
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

    /// 以当前页表为模板创建用户页表：复制内核高半区映射，低半区留空。
    pub fn create_user(&self, allocator: &mut FrameAllocator) -> Result<Self, PagingError> {
        let root = allocator.allocate().ok_or(PagingError::OutOfFrames)?;
        self.zero_frame(root);

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

    /// 切换到该页表。
    pub fn activate(&self) {
        // SAFETY: 根物理地址来自页表或帧分配器；写入 CR3 只影响当前 CPU。
        unsafe {
            asm!("mov cr3, {}", in(reg) self.root.get(), options(nostack, preserves_flags));
        }
    }

    /// 复制用户低半区并标记写时复制，返回子页表。
    pub fn fork_user(&self) -> Option<Self> {
        let child_root = take_frame()?;
        self.zero_frame(child_root);

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

            let child = take_frame()?;
            self.zero_frame(child);
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
                let child = take_frame()?;
                self.zero_frame(child);
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

        let Some(frame) = take_frame() else {
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

    /// 建立 4 KiB 映射。
    pub fn map(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        allocator: &mut FrameAllocator,
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
                self.zero_frame(frame);
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

    fn zero_frame(&self, frame: PhysAddr) {
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
        let mut allocator = FrameAllocator::from_range(PAGE_SIZE as u64, 2 * PAGE_SIZE as u64);
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
