//! HMP 对象创建与解析。

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use spin::Mutex;

use super::Kernel;
use crate::error::KernelError;
use crate::object::{KernelObject, ObjectPayload};
use crate::tcb::TcbId;
use hk_hal::{PhysRange, VirtAddr};
use hk_handle::{Handle, Object, ObjectId, ObjectKind, Rights};
use hk_mem::hmp::{
    Capability, CapabilityOp, Capacity, CapacityId, CapacityKind, Frame, FrameBacking, HmpError,
    OwnedFrame, ProviderKind, Region,
};

impl Kernel {
    /// 在指定线程的句柄表中创建物理页；仅测试用于构造 Frame 句柄。
    #[cfg(test)]
    pub fn create_frame(
        &self,
        thread: TcbId,
        frame: Frame,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Frame {
                frame: Mutex::new(Some(frame)),
                capacity: None,
            })),
            rights,
        )
    }

    fn create_frame_owned(
        &self,
        thread: TcbId,
        frame: Frame,
        capacity: Arc<Mutex<Capacity>>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Frame {
                frame: Mutex::new(Some(frame)),
                capacity: Some(capacity),
            })),
            rights,
        )
    }

    /// 返回 Frame 的物理区间、页大小、所属 Capacity、引用数与固定状态。
    pub fn frame_info(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<(PhysRange, usize, CapacityId, u32, bool), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Frame)?;
        let frame = object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let frame = frame.lock();
        let frame = frame
            .as_ref()
            .ok_or(KernelError::Hmp(HmpError::NotMapped))?;
        Ok((
            PhysRange::new(frame.phys(), frame.len() as u64),
            frame.page_size(),
            frame.capacity(),
            // `refs` 字段报告指向该 Frame 的句柄数：Frame 的引用就是句柄。
            object.header().handle_count(),
            frame.is_pinned(),
        ))
    }

    /// 返回 Frame 的物理区间。
    pub fn frame_range(&self, thread: TcbId, handle: Handle) -> Result<PhysRange, KernelError> {
        self.frame_info(thread, handle).map(|(range, ..)| range)
    }

    /// 返回 Frame 当前常驻字节数。
    pub fn frame_resident(&self, thread: TcbId, handle: Handle) -> Result<u64, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Frame)?;
        Ok(object.frame_resident())
    }

    /// 设置 Frame 的固定驻留状态。
    pub fn pin_frame(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Frame)?;
        let frame = object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let mut frame = frame.lock();
        let frame = frame
            .as_mut()
            .ok_or(KernelError::Hmp(HmpError::NotMapped))?;
        if pinned {
            frame.pin();
        } else {
            frame.unpin();
        }
        Ok(())
    }

    /// 内核内部固定 Frame 驻留，用于 DMA 等由内核保证的正确性约束。
    ///
    /// 与 [`Self::pin_frame`] 不同，这里不要求 WRITE：调用方（运行时）已经通过
    /// DeviceGrant 与句柄授权完成检查，固定只是防止页在设备访问期间被回收。
    pub fn pin_frame_internal(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Frame)?;
        let frame = object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let mut frame = frame.lock();
        let frame = frame
            .as_mut()
            .ok_or(KernelError::Hmp(HmpError::NotMapped))?;
        if pinned {
            frame.pin();
        } else {
            frame.unpin();
        }
        Ok(())
    }

    /// 内核内部固定类型化能力驻留，用于 DMA 映射期间阻止回收。
    ///
    /// 不要求 ADMIN：最小权限的服务可能只持有 READ|WRITE 的共享缓冲能力。
    pub fn pin_capability_dma(
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
        if !object.pin_capability_dma(pinned) {
            return Err(KernelError::Hmp(HmpError::NotMapped));
        }
        Ok(())
    }

    /// 增加/释放一次 Region `PINNED` 持有。
    ///
    /// Region 已经由运行时校验 ADMIN；这里只维护 pin 计数，避免一个 Region
    /// 解除映射就清除其他 Region/scanout 的驻留保证。
    pub fn pin_capability_region(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capability)?;
        let capability = object.capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        capability
            .require(CapabilityOp::Pin)
            .map_err(KernelError::Hmp)?;
        if !pinned && object.capability_scanout() {
            return Err(KernelError::ScanoutActive);
        }
        if !object.pin_capability_region(pinned) {
            return Err(KernelError::Hmp(HmpError::NotMapped));
        }
        Ok(())
    }

    /// 把 Frame 归还给 Capacity。
    pub fn free_frame(
        &self,
        thread: TcbId,
        capacity: Handle,
        frame: Handle,
    ) -> Result<(), KernelError> {
        let (capacity_object, _) =
            self.resolve(thread, capacity, Rights::WRITE, ObjectKind::Capacity)?;
        let (frame_object, _) = self.resolve(thread, frame, Rights::WRITE, ObjectKind::Frame)?;
        if frame_object.header().handle_count() > 1 {
            return Err(KernelError::Hmp(HmpError::PermissionDenied));
        }
        let capacity_lock = capacity_object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let mut capacity = capacity_lock.lock();
        let frame_lock = frame_object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let mut slot = frame_lock.lock();
        if slot.is_none() {
            return Err(KernelError::Hmp(HmpError::NotMapped));
        }
        capacity.free(&mut slot).map_err(KernelError::Hmp)?;
        Ok(())
    }

    /// 在指定线程的句柄表中创建物理容量能力。
    pub fn create_capacity(
        &self,
        thread: TcbId,
        capacity: Capacity,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.create_capacity_shared(thread, Arc::new(Mutex::new(capacity)), rights)
    }

    /// 以共享对象在句柄表中创建物理容量能力。
    ///
    /// 启动期建立的 System Capacity 以同一对象授予多个持有者；分配与用量因此
    /// 始终落在同一个 `CapacityId` 上，不会出现并行的第二本账。
    pub fn create_capacity_shared(
        &self,
        thread: TcbId,
        capacity: Arc<Mutex<Capacity>>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Capacity(capacity))),
            rights,
        )
    }

    /// 返回 Capacity 的共享对象；内核侧分配据此保持与句柄相同的归属。
    pub fn capacity_ref(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<Arc<Mutex<Capacity>>, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        object.capacity().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })
    }

    /// 返回 Capacity 的共享对象；仅用于需要分配/归还物理页的调用者。
    pub fn capacity_ref_write(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<Arc<Mutex<Capacity>>, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Capacity)?;
        object.capacity().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })
    }

    /// 返回 Capacity 的峰值用量与软上限。
    pub fn capacity_stats(&self, thread: TcbId, handle: Handle) -> Result<(u64, u64), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let capacity = capacity.lock();
        Ok((capacity.peak(), capacity.soft_limit()))
    }

    /// 返回对象利用率快照；只用于报告。
    pub fn object_ledger(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<hk_mem::hmp::LedgerSnapshot, KernelError> {
        let Some(kind) = self.handle_kind(thread, handle) else {
            return Err(KernelError::Hmp(HmpError::WrongCapability));
        };
        let (object, _) = self.resolve(thread, handle, Rights::READ, kind)?;
        object
            .ledger_snapshot()
            .ok_or(KernelError::Hmp(HmpError::WrongCapability))
    }

    /// 返回类型化能力的常驻字节数。
    pub fn capability_resident(&self, thread: TcbId, handle: Handle) -> Result<u64, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_resident())
    }

    /// 返回 Capacity 的区间、用途、页大小、配额与用量。
    pub fn capacity_info(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<(PhysRange, CapacityKind, usize, u64, u64), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let capacity = capacity.lock();
        // 窗口可能已被 split/merge 改变，`start/len` 报告首个真实窗口。
        let range = capacity
            .windows()
            .first()
            .copied()
            .unwrap_or_else(|| capacity.range());
        Ok((
            range,
            capacity.kind(),
            capacity.page_size(),
            capacity.limit(),
            capacity.used(),
        ))
    }

    /// 返回 Capacity 绑定的 NUMA 节点。
    pub fn capacity_node(&self, thread: TcbId, handle: Handle) -> Result<Option<u8>, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        Ok(capacity.lock().node())
    }

    /// 返回 Capacity 当前持有的全部物理窗口。
    pub fn capacity_windows(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<alloc::vec::Vec<PhysRange>, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        Ok(capacity.lock().windows())
    }

    /// 从 Capacity 尾部切出子 Capacity 并返回句柄。
    pub fn split_capacity(
        &self,
        thread: TcbId,
        handle: Handle,
        len: usize,
        kind: CapacityKind,
        page_size: usize,
        node: Option<u8>,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let child = capacity
            .lock()
            .split_with_page_size_and_node(CapacityId::next(), len, kind, page_size, node)
            .map_err(KernelError::Hmp)?;
        self.create_capacity(
            thread,
            child,
            Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN,
        )
    }

    /// 把一个已清空的子 Capacity 并回父 Capacity。
    ///
    /// 子 Capacity 的 Frame 必须全部归还；合并后子区间回到父 Capacity 的
    /// 可用空间。子 Capacity 句柄仍由调用方持有并负责关闭。
    pub fn merge_capacity(
        &self,
        thread: TcbId,
        parent: Handle,
        child: Handle,
    ) -> Result<(), KernelError> {
        let (parent_object, _) =
            self.resolve(thread, parent, Rights::ADMIN, ObjectKind::Capacity)?;
        let (child_object, _) = self.resolve(thread, child, Rights::ADMIN, ObjectKind::Capacity)?;
        let parent_lock = parent_object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let child_lock = child_object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let mut child_capacity = child_lock.lock();
        parent_lock
            .lock()
            .merge(&mut child_capacity)
            .map_err(KernelError::Hmp)
    }

    /// 从 Capacity 分配 Frame 并返回 Frame 句柄。
    pub fn allocate_frame(
        &self,
        thread: TcbId,
        handle: Handle,
        len: usize,
        node: Option<u8>,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let frame = capacity
            .lock()
            .allocate_on(len, node)
            .map_err(KernelError::Hmp)?;
        self.create_frame_owned(
            thread,
            frame,
            Arc::clone(capacity),
            Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::TRANSFER | Rights::ADMIN,
        )
    }

    /// 在指定线程的句柄表中创建类型化能力。
    pub fn create_capability(
        &self,
        thread: TcbId,
        capability: Capability,
        backing: Option<FrameBacking>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.try_create_capability(thread, capability, backing, rights)
            .map_err(|(error, _)| error)
    }

    /// 创建类型化能力；句柄注册失败时把后备交还调用方。
    pub fn try_create_capability(
        &self,
        thread: TcbId,
        capability: Capability,
        backing: Option<FrameBacking>,
        rights: Rights,
    ) -> Result<Handle, (KernelError, Option<FrameBacking>)> {
        let resident = backing.as_ref().map_or(0, |page| page.len() as u64);
        let object = Arc::new(KernelObject::new(ObjectPayload::Capability {
            capability,
            backing: Mutex::new(backing),
            provider: Mutex::new(None),
            provider_task: Mutex::new(None),
            provider_token: AtomicU64::new(0),
            pages: Mutex::new(BTreeMap::new()),
            reclaiming: Mutex::new(BTreeSet::new()),
            swapped: Mutex::new(BTreeSet::new()),
            resident_peak: AtomicU64::new(resident),
            pinned: AtomicBool::new(false),
            region_pinned: AtomicU32::new(0),
            dma_pinned: AtomicU32::new(0),
            scanout: AtomicBool::new(false),
            version: AtomicU64::new(0),
        }));
        match self.register(thread, Arc::clone(&object), rights) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                let backing = Arc::try_unwrap(object)
                    .ok()
                    .and_then(KernelObject::into_capability_backing);
                Err((error, backing))
            }
        }
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
        kind: ProviderKind,
    ) -> Result<(), KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::ADMIN, ObjectKind::Capability)?;
        let capability = object.capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        if !kind.supports(capability.kind()) {
            return Err(KernelError::Hmp(HmpError::WrongCapability));
        }
        let (_, endpoint_id) =
            self.resolve(thread, endpoint, Rights::READ, ObjectKind::Endpoint)?;
        let task = self.task_of(thread).map_or(thread.get(), |task| task.get());
        object.set_capability_provider(endpoint_id, task);
        Ok(())
    }

    /// 把已有能力重新绑定到新的 Provider 端点。
    pub fn rebind_capability_provider(
        &self,
        capability: ObjectId,
        endpoint: ObjectId,
        task: u64,
    ) -> Result<(), KernelError> {
        let object = self
            .objects
            .lock()
            .get(&capability)
            .and_then(alloc::sync::Weak::upgrade)
            .ok_or(KernelError::StaleObject(capability))?;
        if object.capability().is_none() {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        }
        object.set_capability_provider(endpoint, task);
        Ok(())
    }

    /// 直接按端点对象绑定 Provider，供装载器代客户端完成绑定。
    pub fn bind_capability_provider_id(
        &self,
        thread: TcbId,
        capability: Handle,
        endpoint: ObjectId,
        kind: ProviderKind,
    ) -> Result<(), KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::ADMIN, ObjectKind::Capability)?;
        let capability = object.capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        if !kind.supports(capability.kind()) {
            return Err(KernelError::Hmp(HmpError::WrongCapability));
        }
        let task = self.task_of(thread).map_or(thread.get(), |task| task.get());
        object.set_capability_provider(endpoint, task);
        Ok(())
    }

    /// Provider 按对象标识重新取得自己提供的能力句柄。
    ///
    /// 只允许绑定该能力的 Provider 任务调用；返回句柄只携带映射需要的权利，
    /// 永远不含 ADMIN。句柄是普通引用，最后一位映射者关闭后对象析构，
    /// `PROVIDER_RELEASE` 随之到达。
    pub fn grant_capability(
        &self,
        thread: TcbId,
        id: ObjectId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let object = self
            .objects
            .lock()
            .get(&id)
            .and_then(|weak| weak.upgrade())
            .ok_or(KernelError::StaleObject(id))?;
        object.capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        let provider = self.task_of(thread).map_or(thread.get(), |task| task.get());
        if object.capability_provider_task() != Some(provider) {
            return Err(KernelError::Hmp(HmpError::PermissionDenied));
        }
        let rights = rights & (Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::TRANSFER);
        self.register(thread, object, rights)
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

    /// 返回内核授予该能力 Provider 的回收令牌。
    pub fn capability_provider_token(
        &self,
        thread: TcbId,
        capability: Handle,
    ) -> Result<Option<u64>, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_provider_token())
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

    /// 取出能力内已装入的一页，返回其租约。
    pub fn take_capability_page(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
    ) -> Result<Option<FrameBacking>, KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::WRITE, ObjectKind::Capability)?;
        Ok(object.take_capability_page(index))
    }

    /// 认领一页的在途回收；重复认领返回 `false`。
    pub fn begin_capability_reclaim(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
    ) -> Result<bool, KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::WRITE, ObjectKind::Capability)?;
        Ok(object.begin_capability_reclaim(index))
    }

    /// 判断能力页是否正在等待 Provider 回收确认。
    pub fn capability_page_reclaiming(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
    ) -> Result<bool, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_page_reclaiming(index))
    }

    /// 释放在途回收标记。
    pub fn end_capability_reclaim(&self, thread: TcbId, capability: Handle, index: usize) {
        if let Ok((object, _)) =
            self.resolve(thread, capability, Rights::WRITE, ObjectKind::Capability)
        {
            object.end_capability_reclaim(index);
        }
    }

    /// 判断能力是否固定驻留。
    pub fn capability_pinned(
        &self,
        thread: TcbId,
        capability: Handle,
    ) -> Result<bool, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        if object.capability().is_none() {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        }
        Ok(object.capability_pinned())
    }

    /// 返回能力对象标识，用于判断多个句柄是否指向同一能力。
    pub fn capability_id(
        &self,
        thread: TcbId,
        capability: Handle,
    ) -> Result<ObjectId, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.header().id())
    }

    /// 标记能力页已由 Provider 保存并离开常驻集合。
    pub fn mark_capability_swapped(&self, thread: TcbId, capability: Handle, index: usize) {
        if let Ok((object, _)) =
            self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)
        {
            object.mark_capability_swapped(index);
        }
    }

    /// 清除能力页的已换出标记。
    pub fn clear_capability_swapped(&self, thread: TcbId, capability: Handle, index: usize) {
        if let Ok((object, _)) =
            self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)
        {
            object.clear_capability_swapped(index);
        }
    }

    /// 判断能力页是否已由 Provider 保存且当前不驻留。
    pub fn capability_swapped(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
    ) -> Result<bool, KernelError> {
        let (object, _) = self.resolve(thread, capability, Rights::READ, ObjectKind::Capability)?;
        Ok(object.capability_swapped(index))
    }

    /// 把一页装入能力。
    pub fn set_capability_page(
        &self,
        thread: TcbId,
        capability: Handle,
        index: usize,
        page: FrameBacking,
    ) -> Result<bool, KernelError> {
        let (object, _) =
            self.resolve(thread, capability, Rights::WRITE, ObjectKind::Capability)?;
        Ok(object.set_capability_page(index, page))
    }

    /// 取出 Frame 及其 Capacity 归属，供能力页或 Provider 转移。
    pub fn take_owned_frame(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<OwnedFrame, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Frame)?;
        object
            .take_owned_frame()
            .ok_or(KernelError::Hmp(HmpError::NotMapped))
    }

    /// 把取出的 Frame 放回原对象；只用于 `SYS_FRAME_MAP` 失败回滚。
    pub fn restore_owned_frame(
        &self,
        thread: TcbId,
        handle: Handle,
        owned: OwnedFrame,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Frame)?;
        object
            .restore_owned_frame(owned)
            .then_some(())
            .ok_or(KernelError::Hmp(HmpError::NotMapped))
    }

    /// 取出类型化能力的常驻后备；失败时由调用方关闭该能力。
    pub fn take_capability_backing(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<Option<FrameBacking>, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capability)?;
        Ok(object.take_capability_backing())
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

    /// 提交 Revision 能力，推进版本。
    pub fn commit_capability(&self, thread: TcbId, handle: Handle) -> Result<u64, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Capability)?;
        object.commit_capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })
    }

    /// 设置类型化能力的固定驻留状态；scanout 驻留期间不能解除固定。
    pub fn pin_capability(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capability)?;
        let capability = object.capability().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capability,
        })?;
        // 类型化操作只接受匹配的能力：Revision/TaskImage 没有驻留语义。
        capability
            .require(CapabilityOp::Pin)
            .map_err(KernelError::Hmp)?;
        if !pinned && object.capability_scanout() {
            return Err(KernelError::ScanoutActive);
        }
        object.set_capability_pinned(pinned);
        Ok(())
    }

    /// 设置 Surface 的 scanout 驻留状态；开启时同时固定驻留。
    pub fn scanout_capability(
        &self,
        thread: TcbId,
        handle: Handle,
        enabled: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capability)?;
        let Some(capability) = object.capability() else {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        };
        if !capability.allows(CapabilityOp::Scanout) {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        }
        object.set_capability_scanout(enabled);
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

    /// 按 Region 的 `GROW` 标志扩展其能力范围。
    pub fn grow_region(
        &self,
        thread: TcbId,
        handle: Handle,
        len: usize,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Region)?;
        let region = object.region().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Region,
        })?;
        region.lock().grow(len).map_err(KernelError::Hmp)
    }

    /// 校验一次 Region protect 是否合法，不修改状态。
    pub fn check_protect_region(
        &self,
        thread: TcbId,
        handle: Handle,
        rights: Rights,
        flags: hk_mem::hmp::RegionFlags,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Region)?;
        let region = object.region().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Region,
        })?;
        region
            .lock()
            .check_protect(rights, flags)
            .map_err(KernelError::Hmp)
    }

    /// 修改 Region 的权利与标志；能力层校验 W^X 与 sealed 约束。
    pub fn protect_region(
        &self,
        thread: TcbId,
        handle: Handle,
        rights: Rights,
        flags: hk_mem::hmp::RegionFlags,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Region)?;
        let region = object.region().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Region,
        })?;
        region
            .lock()
            .protect(rights, flags)
            .map_err(KernelError::Hmp)
    }
}
