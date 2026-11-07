//! Region：类型化能力在 VSpace 中的唯一映射视图。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::VirtAddr;
use hk_handle::{ObjectId, Rights};

use super::capability::Mappable;
use super::{Capability, CapabilityKind, HmpError};

static NEXT_REGION_ID: AtomicU64 = AtomicU64::new(1);

bitflags::bitflags! {
    /// Region 标志。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct RegionFlags: u32 {
        /// 写时复制。
        const COW = hk_syscall::REGION_FLAG_COW;
        /// guard 页。
        const GUARD = hk_syscall::REGION_FLAG_GUARD;
        /// 固定驻留。
        const PINNED = hk_syscall::REGION_FLAG_PINNED;
        /// 只读共享。
        const SEALED = hk_syscall::REGION_FLAG_SEALED;
        /// fork 时不复制。
        const DONT_FORK = hk_syscall::REGION_FLAG_DONT_FORK;
        /// 可按需增长。
        const GROW = hk_syscall::REGION_FLAG_GROW;
    }
}

/// Region 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(u64);

impl RegionId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 类型化能力在 VSpace 中的映射。
pub struct Region {
    id: RegionId,
    capability: Capability,
    offset: u64,
    len: usize,
    rights: Rights,
    flags: RegionFlags,
    virt: Option<VirtAddr>,
}

impl Region {
    /// 创建 Region，校验范围、对齐、W^X 与 sealed 约束。
    pub fn new(
        capability: &Capability,
        offset: u64,
        len: usize,
        rights: Rights,
        flags: RegionFlags,
    ) -> Result<Self, HmpError> {
        let page = capability.page_size() as u64;
        if len == 0 {
            return Err(HmpError::InvalidSize);
        }
        if !offset.is_multiple_of(page) || !(len as u64).is_multiple_of(page) {
            return Err(HmpError::Unaligned);
        }
        let end = offset.checked_add(len as u64).ok_or(HmpError::OutOfRange)?;
        if end > capability.size() {
            return Err(HmpError::OutOfRange);
        }
        let allowed = Rights::READ | Rights::WRITE | Rights::EXECUTE;
        if rights.is_empty() || !allowed.covers(rights) {
            return Err(HmpError::PermissionDenied);
        }
        if rights.contains(Rights::WRITE) && rights.contains(Rights::EXECUTE) {
            return Err(HmpError::WxConflict);
        }
        // 任务镜像区域 sealed：只能只读或可执行，不能写入。
        if capability.kind() == CapabilityKind::TaskImage && rights.contains(Rights::WRITE) {
            return Err(HmpError::ReadOnly);
        }
        if flags.contains(RegionFlags::SEALED) && rights.contains(Rights::WRITE) {
            return Err(HmpError::ReadOnly);
        }
        if flags.contains(RegionFlags::COW) && !rights.contains(Rights::WRITE) {
            return Err(HmpError::PermissionDenied);
        }
        if flags.contains(RegionFlags::COW) && flags.contains(RegionFlags::SEALED) {
            return Err(HmpError::ReadOnly);
        }
        Ok(Self {
            id: RegionId::new(NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed)),
            capability: capability.clone(),
            offset,
            len,
            rights,
            flags,
            virt: None,
        })
    }

    /// 建立虚拟映射。
    pub fn map(&mut self, virt: VirtAddr) -> Result<(), HmpError> {
        if !virt.is_page_aligned() {
            return Err(HmpError::Unaligned);
        }
        if self.virt.is_some() {
            return Err(HmpError::AlreadyMapped);
        }
        self.virt = Some(virt);
        Ok(())
    }

    /// 为 fork 创建子 Region：复制范围与权利，可写页标 COW，并映射到给定地址。
    pub fn forked(&self, virt: VirtAddr) -> Result<Self, HmpError> {
        let flags = if self.rights.contains(Rights::WRITE) {
            self.flags | RegionFlags::COW
        } else {
            self.flags
        };
        let mut region = Self {
            id: RegionId::new(NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed)),
            capability: self.capability.clone(),
            offset: self.offset,
            len: self.len,
            rights: self.rights,
            flags,
            virt: None,
        };
        region.map(virt)?;
        Ok(region)
    }

    /// 标记可写 Region 为 COW。
    pub fn mark_cow(&mut self) {
        if self.rights.contains(Rights::WRITE) {
            self.flags.insert(RegionFlags::COW);
        }
    }

    /// 解除虚拟映射，返回原虚拟地址。
    pub fn unmap(&mut self) -> Result<VirtAddr, HmpError> {
        self.virt.take().ok_or(HmpError::NotMapped)
    }

    /// 修改权利与标志。
    pub fn protect(&mut self, rights: Rights, flags: RegionFlags) -> Result<(), HmpError> {
        let allowed = Rights::READ | Rights::WRITE | Rights::EXECUTE;
        if rights.is_empty() || !allowed.covers(rights) {
            return Err(HmpError::PermissionDenied);
        }
        if rights.contains(Rights::WRITE) && rights.contains(Rights::EXECUTE) {
            return Err(HmpError::WxConflict);
        }
        if self.capability.kind() == CapabilityKind::TaskImage && rights.contains(Rights::WRITE) {
            return Err(HmpError::ReadOnly);
        }
        if flags.contains(RegionFlags::SEALED) && rights.contains(Rights::WRITE) {
            return Err(HmpError::ReadOnly);
        }
        self.rights = rights;
        self.flags = flags;
        Ok(())
    }

    /// 返回 Region 标识。
    pub const fn id(&self) -> RegionId {
        self.id
    }

    /// 返回能力。
    pub const fn capability(&self) -> &Capability {
        &self.capability
    }

    /// 返回能力标识。
    pub const fn capability_id(&self) -> ObjectId {
        self.capability.id()
    }

    /// 返回能力类型。
    pub const fn kind(&self) -> CapabilityKind {
        self.capability.kind()
    }

    /// 返回能力内偏移。
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// 返回字节长度。
    pub const fn len(&self) -> usize {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回权利。
    pub const fn rights(&self) -> Rights {
        self.rights
    }

    /// 返回标志。
    pub const fn flags(&self) -> RegionFlags {
        self.flags
    }

    /// 返回虚拟地址。
    pub const fn virt(&self) -> Option<VirtAddr> {
        self.virt
    }
}

impl Mappable for Region {
    fn capability(&self) -> &Capability {
        &self.capability
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_handle::ObjectId;

    fn capability(kind: CapabilityKind, size: u64) -> Capability {
        Capability::with_id(ObjectId::new(1), kind, size, 4096, None).expect("能力")
    }

    #[test]
    fn validates_range_and_alignment() {
        let capability = capability(CapabilityKind::Anonymous, 0x4000);
        assert_eq!(
            Region::new(&capability, 0, 0, Rights::READ, RegionFlags::empty()).err(),
            Some(HmpError::InvalidSize)
        );
        assert_eq!(
            Region::new(
                &capability,
                0x100,
                0x1000,
                Rights::READ,
                RegionFlags::empty()
            )
            .err(),
            Some(HmpError::Unaligned)
        );
        assert_eq!(
            Region::new(
                &capability,
                0x3000,
                0x2000,
                Rights::READ,
                RegionFlags::empty()
            )
            .err(),
            Some(HmpError::OutOfRange)
        );
        assert!(Region::new(&capability, 0, 0x4000, Rights::READ, RegionFlags::empty()).is_ok());
    }

    #[test]
    fn enforces_wx_and_sealed() {
        let capability = capability(CapabilityKind::Revision, 0x2000);
        assert_eq!(
            Region::new(
                &capability,
                0,
                0x1000,
                Rights::READ | Rights::WRITE | Rights::EXECUTE,
                RegionFlags::empty()
            )
            .err(),
            Some(HmpError::WxConflict)
        );
        assert_eq!(
            Region::new(
                &capability,
                0,
                0x1000,
                Rights::READ | Rights::WRITE,
                RegionFlags::SEALED
            )
            .err(),
            Some(HmpError::ReadOnly)
        );
    }

    #[test]
    fn task_image_regions_are_sealed() {
        let capability = capability(CapabilityKind::TaskImage, 0x2000);
        assert_eq!(
            Region::new(
                &capability,
                0,
                0x1000,
                Rights::READ | Rights::WRITE,
                RegionFlags::empty()
            )
            .err(),
            Some(HmpError::ReadOnly)
        );
        assert!(
            Region::new(
                &capability,
                0,
                0x1000,
                Rights::READ | Rights::EXECUTE,
                RegionFlags::empty()
            )
            .is_ok()
        );
    }

    #[test]
    fn maps_and_unmaps_once() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let mut region = Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::empty())
            .expect("Region");
        assert_eq!(region.map(VirtAddr::new(0x40_0000)), Ok(()));
        assert_eq!(
            region.map(VirtAddr::new(0x41_0000)),
            Err(HmpError::AlreadyMapped)
        );
        assert_eq!(region.unmap(), Ok(VirtAddr::new(0x40_0000)));
        assert_eq!(region.unmap(), Err(HmpError::NotMapped));
    }

    #[test]
    fn fork_copies_region_and_marks_cow() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let region = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        )
        .expect("Region");
        let child = region
            .forked(VirtAddr::new(0x50_0000))
            .expect("forked Region");
        assert_eq!(child.virt(), Some(VirtAddr::new(0x50_0000)));
        assert!(child.flags().contains(RegionFlags::COW));
        assert_eq!(child.capability().id(), region.capability().id());
    }

    #[test]
    fn cow_requires_write_intent() {
        let capability = capability(CapabilityKind::Anonymous, 0x1000);
        assert_eq!(
            Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::COW).err(),
            Some(HmpError::PermissionDenied)
        );
        assert!(
            Region::new(
                &capability,
                0,
                0x1000,
                Rights::READ | Rights::WRITE,
                RegionFlags::COW
            )
            .is_ok()
        );
    }
}
