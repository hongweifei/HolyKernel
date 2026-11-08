//! 页表项属性，以及 CoW 与检查点恢复的属性策略。

use hk_mem::Rights;
use hk_mem::hmp::RegionFlags;

pub(crate) const PRESENT: u64 = 1 << 0;
pub(crate) const WRITABLE: u64 = 1 << 1;
pub(crate) const USER: u64 = 1 << 2;
pub(crate) const PWT: u64 = 1 << 3;
pub(crate) const PCD: u64 = 1 << 4;
pub(crate) const HUGE: u64 = 1 << 7;
pub(crate) const COW: u64 = 1 << 9;
pub(crate) const NX: u64 = 1 << 63;
pub(crate) const FLAGS_MASK: u64 = 0xFFF | NX;

/// 页表项属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// 内核可读写页。
    pub const KERNEL_RW: Self = Self(WRITABLE);
    /// 用户可读写页。
    pub const USER_RW: Self = Self(WRITABLE | USER);
    /// 设备 MMIO 页：可写、写直达、禁止缓存、不可执行。
    pub const MMIO: Self = Self(WRITABLE | PWT | PCD | NX);
    /// 用户可访问的设备 MMIO 页：不可执行。
    pub const USER_MMIO: Self = Self(WRITABLE | USER | PWT | PCD | NX);

    /// 按 Region 权利与标志构造用户页属性。
    ///
    /// 写权限来自 `rights`；只要不带 `EXECUTE` 就设置 NX。`COW` 标志把页映射
    /// 为只读并置位 COW 标记，写时由缺页处理复制。
    pub fn user(rights: Rights, flags: RegionFlags) -> Self {
        let mut bits = USER;
        let cow = flags.contains(RegionFlags::COW);
        if rights.contains(Rights::WRITE) && !cow {
            bits |= WRITABLE;
        }
        if !rights.contains(Rights::EXECUTE) {
            bits |= NX;
        }
        if cow {
            bits |= COW;
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

/// 把快照属性转换为恢复用属性：清除 COW 与大页位，COW 页恢复为可写私有页。
pub const fn restore_flags(bits: u64) -> PageFlags {
    let writable = if bits & COW != 0 {
        WRITABLE
    } else {
        bits & WRITABLE
    };
    PageFlags((bits & !COW & !HUGE) | writable | USER)
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
    fn cow_regions_map_read_only_with_marker() {
        let cow = PageFlags::user(Rights::READ | Rights::WRITE, RegionFlags::COW);
        assert_eq!(cow.raw() & WRITABLE, 0);
        assert_eq!(cow.raw() & COW, COW);
        assert_eq!(cow.raw() & USER, USER);
    }

    #[test]
    fn device_pages_are_not_executable() {
        assert_eq!(PageFlags::MMIO.raw() & NX, NX);
        assert_eq!(PageFlags::USER_MMIO.raw() & NX, NX);
    }

    #[test]
    fn restore_makes_cow_pages_writable_and_clears_markers() {
        let restored = restore_flags(PRESENT | COW | HUGE | USER);
        assert_eq!(restored.raw() & WRITABLE, WRITABLE);
        assert_eq!(restored.raw() & COW, 0);
        assert_eq!(restored.raw() & HUGE, 0);
        assert_eq!(restored.raw() & USER, USER);
    }
}
