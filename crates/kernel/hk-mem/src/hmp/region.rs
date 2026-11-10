//! Region：类型化能力在 Arena 中的唯一映射视图。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::VirtAddr;
use hk_handle::{ObjectId, Rights};

use super::capability::Mappable;
use super::{Capability, CapabilityKind, CapabilityOp, HmpError, LedgerSnapshot};

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

/// 类型化能力在 Arena 中的映射。
pub struct Region {
    id: RegionId,
    capability: Capability,
    offset: u64,
    len: usize,
    rights: Rights,
    flags: RegionFlags,
    virt: Option<VirtAddr>,
    peak: u64,
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
        capability.require(CapabilityOp::Map)?;
        validate(capability, rights, flags)?;
        Ok(Self {
            id: RegionId::new(NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed)),
            capability: capability.clone(),
            offset,
            len,
            rights,
            flags,
            virt: None,
            peak: 0,
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
        self.peak = self.peak.max(self.len as u64);
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
            peak: 0,
        };
        region.map(virt)?;
        Ok(region)
    }

    /// 按 `GROW` 标志扩展 Region 的能力范围。
    ///
    /// 只允许在同一能力内向后增长；虚拟地址与页表扩展由运行时完成。
    pub fn grow(&mut self, len: usize) -> Result<(), HmpError> {
        if !self.flags.contains(RegionFlags::GROW) {
            return Err(HmpError::PermissionDenied);
        }
        self.capability.require(CapabilityOp::Grow)?;
        if self.virt.is_none() {
            return Err(HmpError::NotMapped);
        }
        let page = self.capability.page_size() as u64;
        if len <= self.len || !(len as u64).is_multiple_of(page) {
            return Err(HmpError::InvalidSize);
        }
        let end = self
            .offset
            .checked_add(len as u64)
            .ok_or(HmpError::OutOfRange)?;
        if end > self.capability.size() {
            return Err(HmpError::OutOfRange);
        }
        self.len = len;
        self.peak = self.peak.max(len as u64);
        Ok(())
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
    ///
    /// SEALED 与 COW 都是单向状态：不能通过 protect 清除。
    pub fn protect(&mut self, rights: Rights, flags: RegionFlags) -> Result<(), HmpError> {
        if self.flags.contains(RegionFlags::SEALED) && !flags.contains(RegionFlags::SEALED) {
            return Err(HmpError::ReadOnly);
        }
        if self.flags.contains(RegionFlags::COW) && !flags.contains(RegionFlags::COW) {
            return Err(HmpError::PermissionDenied);
        }
        validate(&self.capability, rights, flags)?;
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

    /// 返回 Region 映射利用率的只读快照；只用于报告，不执行硬配额。
    pub fn ledger_snapshot(&self) -> LedgerSnapshot {
        let charged = if self.virt.is_some() {
            self.len as u64
        } else {
            0
        };
        LedgerSnapshot::utilization(charged, self.capability.size(), self.peak, 0)
    }
}

/// Region 创建与 protect 共用的权利/标志校验。
fn validate(capability: &Capability, rights: Rights, flags: RegionFlags) -> Result<(), HmpError> {
    let allowed = Rights::READ | Rights::WRITE | Rights::EXECUTE;
    if rights.is_empty() || !allowed.covers(rights) {
        return Err(HmpError::PermissionDenied);
    }
    if rights.contains(Rights::WRITE) && rights.contains(Rights::EXECUTE) {
        return Err(HmpError::WxConflict);
    }
    if rights.contains(Rights::EXECUTE) && !capability.allows(CapabilityOp::Execute) {
        return Err(HmpError::WrongCapability);
    }
    // 任务镜像与不可变修订都需要 sealed：只能只读或可执行，不能写入。
    if matches!(
        capability.kind(),
        CapabilityKind::TaskImage | CapabilityKind::Revision
    ) && rights.contains(Rights::WRITE)
    {
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
    Ok(())
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
    fn revision_regions_are_sealed() {
        let capability = capability(CapabilityKind::Revision, 0x2000);
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
        assert!(Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::empty()).is_ok());
        let mut region = Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::empty())
            .expect("Region");
        assert_eq!(
            region.protect(Rights::READ | Rights::WRITE, RegionFlags::empty()),
            Err(HmpError::ReadOnly)
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
    fn grows_only_with_grow_flag() {
        let capability = capability(CapabilityKind::Anonymous, 0x4000);
        let mut region = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::GROW,
        )
        .expect("Region");
        region.map(VirtAddr::new(0x40_0000)).expect("映射");
        assert_eq!(region.grow(0x2000), Ok(()));
        assert_eq!(region.len(), 0x2000);
        assert_eq!(region.grow(0x1000), Err(HmpError::InvalidSize));
        assert_eq!(region.grow(0x5000), Err(HmpError::OutOfRange));

        let mut fixed = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        )
        .expect("Region");
        fixed.map(VirtAddr::new(0x50_0000)).expect("映射");
        assert_eq!(fixed.grow(0x2000), Err(HmpError::PermissionDenied));
    }

    #[test]
    fn execute_requires_executable_capability() {
        let anonymous = capability(CapabilityKind::Anonymous, 0x1000);
        assert_eq!(
            Region::new(
                &anonymous,
                0,
                0x1000,
                Rights::READ | Rights::EXECUTE,
                RegionFlags::empty()
            )
            .err(),
            Some(HmpError::WrongCapability)
        );

        let image = capability(CapabilityKind::TaskImage, 0x1000);
        assert!(
            Region::new(
                &image,
                0,
                0x1000,
                Rights::READ | Rights::EXECUTE,
                RegionFlags::empty()
            )
            .is_ok()
        );

        let frame = capability(CapabilityKind::Frame, 0x1000);
        assert!(
            Region::new(
                &frame,
                0,
                0x1000,
                Rights::READ | Rights::EXECUTE,
                RegionFlags::empty()
            )
            .is_ok()
        );
    }

    #[test]
    fn protect_rejects_cow_without_write() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let mut region = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        )
        .expect("Region");
        region.map(VirtAddr::new(0x40_0000)).expect("映射");
        assert_eq!(
            region.protect(Rights::READ, RegionFlags::COW),
            Err(HmpError::PermissionDenied)
        );
    }

    #[test]
    fn protect_rejects_sealed_removal() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let mut region =
            Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::SEALED).expect("Region");
        region.map(VirtAddr::new(0x40_0000)).expect("映射");
        assert_eq!(
            region.protect(Rights::READ, RegionFlags::empty()),
            Err(HmpError::ReadOnly)
        );
    }

    #[test]
    fn protect_rejects_cow_sealed() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let mut region = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::COW,
        )
        .expect("Region");
        region.map(VirtAddr::new(0x40_0000)).expect("映射");
        assert_eq!(
            region.protect(
                Rights::READ | Rights::WRITE,
                RegionFlags::COW | RegionFlags::SEALED
            ),
            Err(HmpError::ReadOnly)
        );
    }

    #[test]
    fn protect_rejects_execute_on_anonymous() {
        let capability = capability(CapabilityKind::Anonymous, 0x2000);
        let mut region = Region::new(
            &capability,
            0,
            0x1000,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        )
        .expect("Region");
        region.map(VirtAddr::new(0x40_0000)).expect("映射");
        assert_eq!(
            region.protect(Rights::READ | Rights::EXECUTE, RegionFlags::empty()),
            Err(HmpError::WrongCapability)
        );
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
