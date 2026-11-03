//! 物理地址、虚拟地址与物理区间。

use core::fmt;

/// 页内偏移位数。
pub const PAGE_SHIFT: u32 = 12;

/// 页大小，单位为字节。
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// 页内偏移掩码。
const PAGE_MASK: u64 = (PAGE_SIZE as u64) - 1;

macro_rules! define_address {
    ($name:ident, $summary:literal) => {
        #[doc = $summary]
        #[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// 以字节为单位构造地址。
            pub const fn new(bytes: u64) -> Self {
                Self(bytes)
            }

            /// 返回地址的字节数值。
            pub const fn get(self) -> u64 {
                self.0
            }

            /// 向下对齐到页边界。
            pub const fn align_down(self) -> Self {
                Self(self.0 & !PAGE_MASK)
            }

            /// 向上对齐到页边界。
            pub const fn align_up(self) -> Self {
                Self(self.0.saturating_add(PAGE_MASK) & !PAGE_MASK)
            }

            /// 判断地址是否页对齐。
            pub const fn is_page_aligned(self) -> bool {
                self.0 & PAGE_MASK == 0
            }

            /// 返回该地址所在页的页号。
            pub const fn page_index(self) -> u64 {
                self.0 >> PAGE_SHIFT
            }

            /// 按字节偏移地址。
            pub const fn offset(self, bytes: u64) -> Self {
                Self(self.0 + bytes)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}({:#018x})", stringify!($name), self.0)
            }
        }
    };
}

define_address!(PhysAddr, "物理地址，以字节为单位。");
define_address!(VirtAddr, "虚拟地址，以字节为单位。");

/// 半开物理区间 `[start, start + len)`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRange {
    start: PhysAddr,
    len: u64,
}

impl PhysRange {
    /// 构造物理区间。
    pub const fn new(start: PhysAddr, len: u64) -> Self {
        Self { start, len }
    }

    /// 起始物理地址。
    pub const fn start(self) -> PhysAddr {
        self.start
    }

    /// 区间长度，单位为字节。
    pub const fn len(self) -> u64 {
        self.len
    }

    /// 结束物理地址，不含该地址。
    pub const fn end(self) -> PhysAddr {
        PhysAddr::new(self.start.get() + self.len)
    }

    /// 判断区间是否为空。
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// 判断物理地址是否落在区间内。
    pub const fn contains(self, address: PhysAddr) -> bool {
        address.get() >= self.start.get() && address.get() < self.end().get()
    }

    /// 判断两个区间是否相交。
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.get() < other.end().get() && other.start.get() < self.end().get()
    }
}
