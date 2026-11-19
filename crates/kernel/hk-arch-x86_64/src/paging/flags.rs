//! 页表项属性，以及检查点恢复的属性策略。
//!
//! 硬件位来自 `page_table_entry` 的 x86_64 `PTF`，权限到位的转换使用其
//! `MappingFlags`；COW 共享状态由 `hk-paging::FrameRefs` 维护。

use hk_hal::{CachePolicy, MapFlags};
use hk_mem::Rights;
use hk_mem::hmp::RegionFlags;
use page_table_entry::MappingFlags;
use page_table_entry::x86_64::PTF;

pub(crate) const PRESENT: u64 = PTF::PRESENT.bits();
pub(crate) const WRITABLE: u64 = PTF::WRITABLE.bits();
pub(crate) const USER: u64 = PTF::USER_ACCESSIBLE.bits();
pub(crate) const PWT: u64 = PTF::WRITE_THROUGH.bits();
pub(crate) const PCD: u64 = PTF::NO_CACHE.bits();
/// 4 KiB 页表项第 7 位是 PAT 选择位，与 PCD/PWT 组成 PAT 索引。
pub(crate) const PAT: u64 = 1 << 7;
/// 设备页判定位：命中任一即表示该页不是普通用户数据页。
///
/// 页表遍历与写时复制共用这一组位跳过设备映射，新增缓存属性时只改这里。
pub(crate) const DEVICE: u64 = PWT | PCD | PAT;
pub(crate) const HUGE: u64 = PTF::HUGE_PAGE.bits();
pub(crate) const NX: u64 = PTF::NO_EXECUTE.bits();
pub(crate) const FLAGS_MASK: u64 = 0xFFF | NX;

/// 缓存策略在 PAT 表中的索引：`(PAT << 2) | (PCD << 1) | PWT`。
///
/// 3=UC、4=WB、6=WC；`cpu::assembly::enable_pat` 用同一函数建表，索引与表项
/// 不会各写一份。
pub(crate) const fn cache_index(policy: CachePolicy) -> u32 {
    match policy {
        CachePolicy::Uncached => 3,
        CachePolicy::WriteBack => 4,
        CachePolicy::WriteCombining => 6,
    }
}

/// PAT 索引到 4 KiB 页表项的控制位。
pub(crate) const fn index_bits(index: u32) -> u64 {
    let mut bits = 0;
    if index & 0b100 != 0 {
        bits |= PAT;
    }
    if index & 0b010 != 0 {
        bits |= PCD;
    }
    if index & 0b001 != 0 {
        bits |= PWT;
    }
    bits
}

/// 每种策略的页表位，下标即 [`CachePolicy`] 的判别值。
const PAT_BITS: [u64; 3] = [
    index_bits(cache_index(CachePolicy::WriteBack)),
    index_bits(cache_index(CachePolicy::WriteCombining)),
    index_bits(cache_index(CachePolicy::Uncached)),
];

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
    /// 设备 MMIO 页：可写、非缓存、不可执行。
    pub const MMIO: Self = Self(PRESENT | WRITABLE | NX | PAT_BITS[CachePolicy::Uncached as usize]);
    /// 用户设备页，缓存属性按 [`CachePolicy`] 选 PAT 索引。
    ///
    /// 设备页始终 NX；写权限来自 Region 的 Rights，只读设备映射不会被
    /// 悄悄提升为可写。
    pub const fn device(policy: CachePolicy, rights: Rights) -> Self {
        let mut bits = PRESENT | USER | NX | PAT_BITS[policy as usize];
        if rights.bits() & Rights::WRITE.bits() != 0 {
            bits |= WRITABLE;
        }
        Self(bits)
    }

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
    /// 始终设置 `USER` 与 `PRESENT`；`DEVICE` 表示非缓存设备页，位组合来自
    /// [`CachePolicy::Uncached`]，与设备映射共用同一份索引逻辑。
    pub fn from_map(flags: MapFlags) -> Self {
        let mut bits = PRESENT | USER;
        if flags.contains(MapFlags::WRITE) {
            bits |= WRITABLE;
        }
        if !flags.contains(MapFlags::EXECUTE) {
            bits |= NX;
        }
        if flags.contains(MapFlags::DEVICE) {
            bits |= PAT_BITS[CachePolicy::Uncached as usize] | NX;
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
    let shared = bits & hk_base::PAGE_FLAG_SHARED != 0;
    let writable = if shared { WRITABLE } else { bits & WRITABLE };
    PageFlags((bits & !HUGE & !hk_base::PAGE_FLAG_SHARED) | writable | USER)
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
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
        for policy in [
            CachePolicy::WriteBack,
            CachePolicy::WriteCombining,
            CachePolicy::Uncached,
        ] {
            assert_eq!(
                PageFlags::device(policy, Rights::READ | Rights::WRITE).raw() & NX,
                NX
            );
        }
    }

    #[test]
    fn cache_policy_selects_device_pat_index() {
        // 索引 3=UC、4=WB、6=WC 由 `enable_pat` 固定；位组合错就是缓存属性错。
        let uncached = PageFlags::device(CachePolicy::Uncached, Rights::READ | Rights::WRITE).raw();
        assert_eq!(uncached & DEVICE, PWT | PCD);
        let write_back =
            PageFlags::device(CachePolicy::WriteBack, Rights::READ | Rights::WRITE).raw();
        assert_eq!(write_back & DEVICE, PAT);
        let combining =
            PageFlags::device(CachePolicy::WriteCombining, Rights::READ | Rights::WRITE).raw();
        assert_eq!(combining & DEVICE, PAT | PCD);
        assert_eq!(
            write_back & (PRESENT | WRITABLE | USER),
            PRESENT | WRITABLE | USER
        );
        assert_eq!(
            PageFlags::device(CachePolicy::WriteBack, Rights::READ).raw() & WRITABLE,
            0,
            "只读设备映射不得提升为可写"
        );
    }

    #[test]
    fn restore_makes_shared_pages_writable_and_clears_markers() {
        let restored = restore_flags(PRESENT | hk_base::PAGE_FLAG_SHARED | HUGE | USER);
        assert_eq!(restored.raw() & WRITABLE, WRITABLE);
        assert_eq!(restored.raw() & hk_base::PAGE_FLAG_SHARED, 0);
        assert_eq!(restored.raw() & HUGE, 0);
        assert_eq!(restored.raw() & USER, USER);
    }
}
