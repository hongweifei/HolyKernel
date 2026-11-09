//! 写时复制：fork 时共享可写页，缺页时复制页帧并恢复写权限。

use hk_hal::{PAGE_SIZE, PhysAddr};
use hk_mem::page::PagePool;

use super::FRAME_BANK;
use super::flags::{COW, HUGE, PRESENT, WRITABLE};
use super::table::{ADDRESS_MASK, ENTRIES, KERNEL_PML4_START, PageTable};

fn take_cow_page() -> Option<PhysAddr> {
    PagePool::new(&FRAME_BANK).cow()
}

impl PageTable {
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
        unsafe { self.entry(table, index).write_volatile(updated) };
        hk_arch::flush_tlb_page(address);
        true
    }
}
