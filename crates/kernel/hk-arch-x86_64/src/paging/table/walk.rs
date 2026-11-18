//! 页表遍历：用户页枚举、内核高半区登记与地址翻译。

use alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};

use crate::paging::flags::{DEVICE, FLAGS_MASK, HUGE, PCD, PRESENT, PWT, USER};

use super::{ADDRESS_MASK, ENTRIES, KERNEL_PML4_START, PageTable};

/// 把高半区位模式还原为规范符号扩展地址。
fn canonical_high_half(virt: u64) -> u64 {
    virt | 0xFFFF_0000_0000_0000
}

impl PageTable {
    /// 遍历低半区的 4 KiB 用户页；大页展开为 4 KiB。
    ///
    /// 设备页按 `DEVICE` 位跳过：4 KiB 页为 `PWT | PCD | PAT`，大页只有
    /// `PWT | PCD` 可用（大页的第 7 位是页大小标志，不是 PAT）。
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
                let pdpt_virt = ((pml4 as u64) << 39) | ((pdpt_index as u64) << 30);
                if pdp & HUGE != 0 {
                    if pdp & (PWT | PCD) != 0 {
                        continue;
                    }
                    let base = pdp & ADDRESS_MASK;
                    for pd_index in 0..ENTRIES {
                        for pt_index in 0..ENTRIES {
                            let virt =
                                pdpt_virt | ((pd_index as u64) << 21) | ((pt_index as u64) << 12);
                            let phys = base + ((pd_index as u64) << 21) + ((pt_index as u64) << 12);
                            visit(virt, pdp & FLAGS_MASK, phys);
                        }
                    }
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & USER == 0 {
                        continue;
                    }
                    let pd_virt = pdpt_virt | ((pd_index as u64) << 21);
                    if pde & HUGE != 0 {
                        if pde & (PWT | PCD) != 0 {
                            continue;
                        }
                        let base = pde & ADDRESS_MASK;
                        for pt_index in 0..ENTRIES {
                            let virt = pd_virt | ((pt_index as u64) << 12);
                            let phys = base + ((pt_index as u64) << 12);
                            visit(virt, pde & FLAGS_MASK, phys);
                        }
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & USER == 0 {
                            continue;
                        }
                        if pte & DEVICE != 0 {
                            continue;
                        }
                        let virt = pd_virt | ((pt_index as u64) << 12);
                        visit(virt, pte & FLAGS_MASK, pte & ADDRESS_MASK);
                    }
                }
            }
        }
    }

    /// 收集低半区用户页表与数据帧，供检查点释放或复用。
    pub fn user_frames(&self) -> Vec<PhysAddr> {
        let mut frames = Vec::new();
        frames.push(self.root);
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            frames.push(pdpt);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                frames.push(pd);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    frames.push(pt);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & DEVICE != 0 {
                            continue;
                        }
                        frames.push(PhysAddr::new(pte & ADDRESS_MASK));
                    }
                }
            }
        }
        frames.sort_unstable();
        frames.dedup();
        frames
    }

    /// 遍历内核高半区的叶子映射，供 System Arena 登记内核 Region。
    ///
    /// 回调参数为 `(虚拟地址, 物理地址, 长度, 原始属性位)`；大页按实际
    /// 尺寸报告。中间页表帧本身不是映射视图，由分配记录单独登记。
    pub fn kernel_half_regions<F>(&self, mut visit: F)
    where
        F: FnMut(u64, u64, u64, u64),
    {
        for pml4 in KERNEL_PML4_START..ENTRIES {
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
                let pdpt_virt =
                    canonical_high_half((pml4 as u64) << 39) | ((pdpt_index as u64) << 30);
                if pdp & HUGE != 0 {
                    visit(pdpt_virt, pdp & ADDRESS_MASK, 1 << 30, pdp & FLAGS_MASK);
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 {
                        continue;
                    }
                    let pd_virt = pdpt_virt | ((pd_index as u64) << 21);
                    if pde & HUGE != 0 {
                        visit(pd_virt, pde & ADDRESS_MASK, 1 << 21, pde & FLAGS_MASK);
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 {
                            continue;
                        }
                        let virt = pd_virt | ((pt_index as u64) << 12);
                        visit(virt, pte & ADDRESS_MASK, PAGE_SIZE as u64, pte & FLAGS_MASK);
                    }
                }
            }
        }
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

    /// 查询虚拟地址对应的物理地址与原始页表属性。
    pub fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, u64)> {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((virt.get() >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 {
                return None;
            }
            if value & HUGE != 0 {
                let size = match level {
                    30 => 1_u64 << 30,
                    21 => 1_u64 << 21,
                    _ => return None,
                };
                let phys = (value & ADDRESS_MASK) + (virt.get() & (size - 1));
                return Some((PhysAddr::new(phys), value & FLAGS_MASK));
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }
        let index = ((virt.get() >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        if value & PRESENT == 0 {
            return None;
        }
        Some((PhysAddr::new(value & ADDRESS_MASK), value & FLAGS_MASK))
    }

    /// 返回 `address` 当前映射的物理页；未映射或大页时返回 `None`。
    pub(crate) fn mapped_phys(&self, address: u64) -> Option<PhysAddr> {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 || value & HUGE != 0 {
                return None;
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }
        let index = ((address >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        (value & PRESENT != 0).then_some(PhysAddr::new(value & ADDRESS_MASK))
    }
}
