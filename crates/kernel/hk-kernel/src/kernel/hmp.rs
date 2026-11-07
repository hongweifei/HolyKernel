//! HMP 对象创建与解析。

use core::sync::atomic::{AtomicBool, AtomicU64};

use super::*;
use hk_hal::{PhysRange, VirtAddr};
use hk_mem::hmp::{Capability, Capacity, Ledger, Region};

impl Kernel {
    /// 在指定线程的句柄表中创建物理容量能力。
    pub fn create_capacity(
        &self,
        thread: TcbId,
        capacity: Capacity,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Capacity(Mutex::new(
                capacity,
            )))),
            rights,
        )
    }

    /// 在指定线程的句柄表中创建类型化能力。
    pub fn create_capability(
        &self,
        thread: TcbId,
        capability: Capability,
        backing: Option<hk_mem::hmp::FrameLease>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let pages = if backing.is_none() {
            core::iter::repeat_with(|| None)
                .take((capability.size() / capability.page_size() as u64) as usize)
                .collect()
        } else {
            alloc::vec::Vec::new()
        };
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Capability {
                capability,
                backing,
                provider: Mutex::new(None),
                pages: Mutex::new(pages),
                pinned: AtomicBool::new(false),
                version: AtomicU64::new(0),
            })),
            rights,
        )
    }

    /// 解析类型化能力，返回能力描述与常驻物理区间。
    pub fn capability(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<(Capability, Option<PhysRange>), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capability)?;
        let capability = object.capability().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        Ok((capability, object.capability_backing()))
    }

    /// 把类型化能力绑定到 Provider 端点。
    pub fn bind_capability_provider(
        &self,
        thread: TcbId,
        capability: Handle,
        endpoint: Handle,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        let (_, endpoint_id) =
            self.resolve(thread, endpoint, Rights::READ, ObjectKind::Endpoint)?;
        object.set_capability_provider(endpoint_id);
        Ok(())
    }

    /// 返回类型化能力绑定的 Provider 端点。
    pub fn capability_provider(
        &self,
        thread: TcbId,
        capability: Handle,
    ) -> Result<Option<ObjectId>, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_provider())
    }

    /// 返回能力内已装入的页。
    pub fn capability_page(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
    ) -> Result<Option<PhysRange>, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_page(index))
    }

    /// 把一页装入能力。
    pub fn set_capability_page(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
        lease: hk_mem::hmp::FrameLease,
    ) -> Result<bool, KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::WRITE, ObjectKind::Capability)?;
        Ok(object.set_capability_page(index, lease))
    }

    /// 在指定线程的句柄表中创建虚拟映射。
    pub fn create_region(
        &self,
        thread: TcbId,
        region: Region,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Region(Mutex::new(region)))),
            rights,
        )
    }

    /// 在指定线程的句柄表中创建配额账本。
    pub fn create_ledger(
        &self,
        thread: TcbId,
        ledger: Ledger,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Ledger(Mutex::new(ledger)))),
            rights,
        )
    }

    /// 提交 Revision 能力，推进版本。
    pub fn commit_capability(&self, thread: TcbId, handle: Handle) -> Result<u64, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capability)?;
        object.commit_capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })
    }

    /// 设置类型化能力的固定驻留状态。
    pub fn pin_capability(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capability)?;
        if object.capability().is_none() {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        }
        object.set_capability_pinned(pinned);
        Ok(())
    }

    /// 为 fork 复制 Region 元数据，并映射到子任务的虚拟地址。
    pub fn fork_region(
        &self,
        thread: TcbId,
        handle: Handle,
        virt: VirtAddr,
    ) -> Result<Region, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Region)?;
        let region = object.region().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Region,
        })?;
        region
            .lock()
            .forked(virt)
            .map_err(|_| KernelError::WrongKind {
                expected: ObjectKind::Region,
            })
    }

    /// 把可写 Region 标记为 COW。
    pub fn mark_region_cow(&self, thread: TcbId, handle: Handle) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Region)?;
        let region = object.region().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Region,
        })?;
        region.lock().mark_cow();
        Ok(())
    }
}
