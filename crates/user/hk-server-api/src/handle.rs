//! 类型化句柄：线协议仍是单一 `u64`，类型只在客户端编译期存在。
//!
//! 与 [ADR-0016](../../../docs/adr/0016-hmp-native-contract.md) 第 6 节一致：
//! `Handle<Revision>`、`Handle<Surface>`、`Handle<Capability>` 等包装是
//! `#[repr(transparent)]` 的零成本视图，不改变句柄表示，也不引入运行时检查。
//! 类型化创建在发起 `SYS_CAPABILITY_CREATE*` 时把种类固定为标签对应的值，
//! 因此 `Handle<Anonymous>` 与内核对象的种类在构造点一致；从裸值构造
//! （[`Handle::from_raw`]）供 ABI 边界使用，调用方需保证种类匹配。

use core::marker::PhantomData;

use crate::CapabilityKind;
use crate::memory::{
    capability_bind_provider, capability_commit, capability_create, capability_create_lazy,
    capability_export, capability_phys, capability_pin, capability_scanout,
};

mod sealed {
    /// 防止外部为 [`super::Handle`] 引入未定义的种类。
    pub trait Sealed {}
}

/// 句柄种类标签。
pub trait Kind: sealed::Sealed {
    /// 种类名称，用于诊断。
    const NAME: &'static str;
}

/// 类型化能力标签：对应内核 `CapabilityKind`。
pub trait CapabilityTag: Kind {
    /// 创建时写入的对象种类。
    const KIND: CapabilityKind;
}

macro_rules! define_kinds {
    ($( $(#[$meta:meta])* $name:ident => $label:literal ),* $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub struct $name;

            impl sealed::Sealed for $name {}

            impl Kind for $name {
                const NAME: &'static str = $label;
            }
        )*
    };
}

define_kinds! {
    /// 同步 IPC 端点。
    Endpoint => "endpoint",
    /// 物理容量能力。
    Capacity => "capacity",
    /// 物理页对象。
    Frame => "frame",
    /// 虚拟映射对象。
    Region => "region",
    /// 种类在运行期决定的类型化能力。
    Capability => "capability",
}

define_kinds! {
    /// 文档修订（只读、不可变）。
    Revision => "revision",
    /// 像素缓冲。
    Surface => "surface",
    /// 任务镜像。
    TaskImage => "task-image",
    /// 设备分配。
    DeviceAllocation => "device-allocation",
    /// 匿名内存。
    Anonymous => "anonymous",
}

impl CapabilityTag for Revision {
    const KIND: CapabilityKind = CapabilityKind::Revision;
}

impl CapabilityTag for Surface {
    const KIND: CapabilityKind = CapabilityKind::Surface;
}

impl CapabilityTag for TaskImage {
    const KIND: CapabilityKind = CapabilityKind::TaskImage;
}

impl CapabilityTag for DeviceAllocation {
    const KIND: CapabilityKind = CapabilityKind::DeviceAllocation;
}

impl CapabilityTag for Anonymous {
    const KIND: CapabilityKind = CapabilityKind::Anonymous;
}

/// 零成本类型化句柄。
#[repr(transparent)]
pub struct Handle<K: Kind> {
    raw: u64,
    kind: PhantomData<fn() -> K>,
}

impl<K: Kind> Handle<K> {
    /// 从 ABI 裸值构造；调用方保证句柄种类与 `K` 匹配。
    pub const fn from_raw(raw: u64) -> Self {
        Self {
            raw,
            kind: PhantomData,
        }
    }

    /// 返回 ABI 裸值。
    pub const fn raw(self) -> u64 {
        self.raw
    }
}

impl<K: Kind> Clone for Handle<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Kind> Copy for Handle<K> {}

impl<K: Kind> core::fmt::Debug for Handle<K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "Handle<{}>({})", K::NAME, self.raw)
    }
}

impl<K: Kind> PartialEq for Handle<K> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<K: Kind> Eq for Handle<K> {}

impl<K: Kind> core::hash::Hash for Handle<K> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<K: Kind> From<Handle<K>> for u64 {
    fn from(handle: Handle<K>) -> Self {
        handle.raw
    }
}

impl<K: Kind> From<u64> for Handle<K> {
    fn from(raw: u64) -> Self {
        Self::from_raw(raw)
    }
}

impl<K: CapabilityTag> Handle<K> {
    /// 申请一段带常驻后备的能力。
    pub fn create(length: usize) -> Option<Self> {
        capability_create(K::KIND, length).map(Self::from_raw)
    }

    /// 申请一段按需装入的能力。
    pub fn create_lazy(length: usize) -> Option<Self> {
        capability_create_lazy(K::KIND, length).map(Self::from_raw)
    }

    /// 绑定 Provider 端点。
    pub fn bind_provider(self, endpoint: Handle<Endpoint>, provider: u8) -> u64 {
        capability_bind_provider(self.raw, endpoint.raw, provider)
    }

    /// 查询首个物理区间。
    pub fn phys(self) -> Option<(u64, u64)> {
        capability_phys(self.raw)
    }

    /// 擦除种类标签，得到运行期种类未知的能力句柄。
    pub fn erase(self) -> Handle<Capability> {
        Handle::from_raw(self.raw)
    }
}

impl Handle<Anonymous> {
    /// 设置固定驻留。
    pub fn pin(self, pinned: bool) -> u64 {
        capability_pin(self.raw(), pinned)
    }
}

impl Handle<Surface> {
    /// 设置 scanout 驻留。
    pub fn scanout(self, enabled: bool) -> u64 {
        capability_scanout(self.raw(), enabled)
    }

    /// 设置固定驻留。
    pub fn pin(self, pinned: bool) -> u64 {
        capability_pin(self.raw(), pinned)
    }
}

impl Handle<Revision> {
    /// 提交修订。
    pub fn commit(self) -> Option<u64> {
        capability_commit(self.raw())
    }

    /// 请求 Provider 回写常驻页。
    pub fn writeback(self) -> u64 {
        crate::memory::capability_writeback(self.raw())
    }
}

impl Handle<DeviceAllocation> {
    /// 导出给其他进程或设备；返回同种类的新句柄。
    pub fn export(self) -> Option<Self> {
        capability_export(self.raw()).map(Self::from_raw)
    }

    /// 设置固定驻留。
    pub fn pin(self, pinned: bool) -> u64 {
        capability_pin(self.raw(), pinned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn typed_handles_are_zero_cost() {
        assert_eq!(size_of::<Handle<Anonymous>>(), size_of::<u64>());
        assert_eq!(align_of::<Handle<Anonymous>>(), align_of::<u64>());
        let handle = Handle::<Revision>::from_raw(0x1234);
        assert_eq!(handle.raw(), 0x1234);
        assert_eq!(handle.erase(), Handle::<Capability>::from_raw(0x1234));
        assert_eq!(u64::from(handle), 0x1234);
    }

    #[test]
    fn kinds_keep_their_capability_encoding() {
        assert_eq!(Anonymous::KIND, CapabilityKind::Anonymous);
        assert_eq!(Surface::KIND, CapabilityKind::Surface);
        assert_eq!(Revision::KIND, CapabilityKind::Revision);
        assert_eq!(TaskImage::KIND, CapabilityKind::TaskImage);
        assert_eq!(DeviceAllocation::KIND, CapabilityKind::DeviceAllocation);
    }
}
