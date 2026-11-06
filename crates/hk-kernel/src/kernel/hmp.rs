//! HMP 对象创建与解析。

use super::*;
use hk_hal::VirtAddr;
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
        backing: Option<hk_mem::Frame>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Capability {
                capability,
                backing,
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
