//! 页表项属性，以及检查点恢复的属性策略。
//!
//! 硬件位来自 `page_table_entry` 的 x86_64 `PTF`，权限到位的转换使用其
//! `MappingFlags`；COW 共享状态由 `hk-paging::FrameRefs` 维护。

use hk_hal::MapFlags;
use hk_mem::Rights;
use hk_mem::hmp::RegionFlags;
use page_table_entry::MappingFlags;
use page_table_entry::x86_64::PTF;

pub(crate) const PRESENT: u64 = PTF::PRESENT.bits();
pub(crate) const WRITABLE: u64 = PTF::WRITABLE.bits();
pub(crate) const USER: u64 = PTF::USER_ACCESSIBLE.bits();
pub(crate) const PWT: u64 = PTF::WRITE_THROUGH.bits();
pub(crate) const PCD: u64 = PTF::NO_CACHE.bits();
pub(crate) const HUGE: u64 = PTF::HUGE_PAGE.bits();
pub(crate) const NX: u64 = PTF::NO_EXECUTE.bits();
pub(crate) const FLAGS_MASK: u64 = 0xFFF | NX;

fn encode(flags: MappingFlags) -> u64 {
    PTF::from(flags).bits()
}

/// 页表项属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// 内核可读写数据页；不可执行。
    pub const KERNEL_RW: Self = Self(PRESENT | WRITABLE | NX);
    /// 用户页的加载器初始属性；随后按段权限收紧。
    pub const USER_RW: Self = Self(PRESENT | WRITABLE | USER);
    /// 内核栈页：可写、不可执行、禁止用户访问。
    pub const KERNEL_STACK: Self = Self(PRESENT | WRITABLE | NX);
    /// 设备 MMIO 页：可写、写直达、禁止缓存、不可执行。
    pub const MMIO: Self = Self(PRESENT | WRITABLE | PWT | PCD | NX);
    /// 用户可访问的设备 MMIO 页：不可执行。
    pub const USER_MMIO: Self = Self(PRESENT | WRITABLE | USER | PWT | PCD | NX);

    /// 按 Region 权利与标志构造用户页属性。
    ///
    /// 写权限来自 `rights`；不带 `EXECUTE` 时由 `MappingFlags` 转出 NX。
    /// `COW` Region 映射为只读，共享引用由 `hk-paging::FrameRefs` 维护。
    pub fn user(rights: Rights, flags: RegionFlags) -> Self {
        let cow = flags.contains(RegionFlags::COW);
        let mut mapping = MappingFlags::READ | MappingFlags::USER;
        if rights.contains(Rights::WRITE) && !cow {
            mapping |= MappingFlags::WRITE;
        }
        if rights.contains(Rights::EXECUTE) {
            mapping |= MappingFlags::EXECUTE;
        }
        Self(encode(mapping))
    }

    /// 按 HAL 映射属性构造用户页属性。
    ///
    /// 始终设置 `USER` 与 `PRESENT`；`DEVICE` 映射为写直达、禁止缓存且不可执行。
    pub fn from_map(flags: MapFlags) -> Self {
        let mut bits = PRESENT | USER;
        if flags.contains(MapFlags::WRITE) {
            bits |= WRITABLE;
        }
        if !flags.contains(MapFlags::EXECUTE) {
            bits |= NX;
        }
        if flags.contains(MapFlags::DEVICE) {
            bits |= PWT | PCD | NX;
        }
        Self(bits)
    }

    pub(crate) const fn bits(self) -> u64 {
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

/// 把快照属性转换为恢复用属性：清除共享标记与大页位，共享页恢复为可写私有页。
pub const fn restore_flags(bits: u64) -> PageFlags {
    let shared = bits & hk_checkpoint::PAGE_FLAG_SHARED != 0;
    let writable = if shared { WRITABLE } else { bits & WRITABLE };
    PageFlags((bits & !HUGE & !hk_checkpoint::PAGE_FLAG_SHARED) | writable | USER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_flags_follow_region_rights() {
        let read_only = PageFlags::user(Rights::READ, RegionFlags::empty());
        assert_eq!(read_only.raw() & WRITABLE, 0);
        assert_eq!(read_only.raw() & NX, NX);

        let writable = PageFlags::user(Rights::READ | Rights::WRITE, RegionFlags::empty());
        assert_eq!(writable.raw() & WRITABLE, WRITABLE);
        assert_eq!(writable.raw() & NX, NX);

        let executable = PageFlags::user(Rights::READ | Rights::EXECUTE, RegionFlags::empty());
        assert_eq!(executable.raw() & WRITABLE, 0);
        assert_eq!(executable.raw() & NX, 0);
    }

    #[test]
    fn cow_regions_map_read_only_without_architecture_marker() {
        let cow = PageFlags::user(Rights::READ | Rights::WRITE, RegionFlags::COW);
        assert_eq!(cow.raw() & WRITABLE, 0);
        assert_eq!(cow.raw() & USER, USER);
    }

    #[test]
    fn map_flags_convert_to_pte_bits() {
        let writable = PageFlags::from_map(MapFlags::READ | MapFlags::WRITE);
        assert_eq!(writable.raw() & PRESENT, PRESENT);
        assert_eq!(writable.raw() & USER, USER);
        assert_eq!(writable.raw() & WRITABLE, WRITABLE);
        assert_eq!(writable.raw() & NX, NX);

        let executable = PageFlags::from_map(MapFlags::READ | MapFlags::EXECUTE);
        assert_eq!(executable.raw() & WRITABLE, 0);
        assert_eq!(executable.raw() & NX, 0);

        let device = PageFlags::from_map(MapFlags::READ | MapFlags::WRITE | MapFlags::DEVICE);
        assert_eq!(device.raw() & (PWT | PCD), PWT | PCD);
        assert_eq!(device.raw() & NX, NX);
    }

    #[test]
    fn device_pages_are_not_executable() {
        assert_eq!(PageFlags::MMIO.raw() & NX, NX);
        assert_eq!(PageFlags::USER_MMIO.raw() & NX, NX);
    }

    #[test]
    fn restore_makes_shared_pages_writable_and_clears_markers() {
        let restored = restore_flags(PRESENT | hk_checkpoint::PAGE_FLAG_SHARED | HUGE | USER);
        assert_eq!(restored.raw() & WRITABLE, WRITABLE);
        assert_eq!(restored.raw() & hk_checkpoint::PAGE_FLAG_SHARED, 0);
        assert_eq!(restored.raw() & HUGE, 0);
        assert_eq!(restored.raw() & USER, USER);
    }
}
