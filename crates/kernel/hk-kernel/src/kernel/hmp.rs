//! HMP 对象创建与解析。

use core::sync::atomic::{AtomicBool, AtomicU64};

use super::*;
use hk_hal::{PhysRange, VirtAddr};
use hk_mem::hmp::{
    Arena, ArenaRegion, Capability, CapabilityOp, Capacity, CapacityId, CapacityKind, Frame,
    FrameBacking, HmpError, Ledger, OwnedFrame, Region, RegionId,
};

impl Kernel {
    /// 在指定线程的句柄表中创建任务内存根。
    pub fn create_arena(
        &self,
        thread: TcbId,
        arena: Arena,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Arena(Mutex::new(arena)))),
            rights,
        )
    }

    /// 在 Arena 中登记 Region。
    pub fn arena_insert_region(
        &self,
        thread: TcbId,
        arena: Handle,
        region: &Region,
    ) -> Result<RegionId, KernelError> {
        let (object, _) = self.resolve(thread, arena, Rights::WRITE, ObjectKind::Arena)?;
        let arena = object.arena().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Arena,
        })?;
        arena.lock().insert(region).map_err(KernelError::Hmp)
    }

    /// 从 Arena 中移除 Region。
    pub fn arena_remove_region(
        &self,
        thread: TcbId,
        arena: Handle,
        region: RegionId,
    ) -> Result<ArenaRegion, KernelError> {
        let (object, _) = self.resolve(thread, arena, Rights::WRITE, ObjectKind::Arena)?;
        let arena = object.arena().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Arena,
        })?;
        arena.lock().remove(region).map_err(KernelError::Hmp)
    }

    /// 返回 Arena 中的 Region 数。
    pub fn arena_region_count(&self, thread: TcbId, arena: Handle) -> Result<usize, KernelError> {
        let (object, _) = self.resolve(thread, arena, Rights::READ, ObjectKind::Arena)?;
        let arena = object.arena().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Arena,
        })?;
        Ok(arena.lock().regions().len())
    }

    /// 在指定线程的句柄表中创建物理页。
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
            frame.refs(),
            frame.is_pinned(),
        ))
    }

    /// 返回 Frame 的物理区间。
    pub fn frame_range(&self, thread: TcbId, handle: Handle) -> Result<PhysRange, KernelError> {
        self.frame_info(thread, handle).map(|(range, ..)| range)
    }

    /// 返回 Frame 的共享引用数。
    pub fn frame_refs(&self, thread: TcbId, handle: Handle) -> Result<u32, KernelError> {
        self.frame_info(thread, handle)
            .map(|(_, _, _, refs, _)| refs)
    }

    /// 增加 Frame 的共享引用。
    pub fn share_frame(&self, thread: TcbId, handle: Handle) -> Result<u32, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Frame)?;
        let frame = object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let mut frame = frame.lock();
        let frame = frame
            .as_mut()
            .ok_or(KernelError::Hmp(HmpError::NotMapped))?;
        frame.share().map_err(KernelError::Hmp)
    }

    /// 释放一次 Frame 共享引用。
    ///
    /// 引用降到零时 Frame 已无所有者，立即归还所属 Capacity 并从对象中取出。
    /// 归还后句柄保留但内容为空，后续释放或关闭不会重复归还。
    pub fn release_frame(&self, thread: TcbId, handle: Handle) -> Result<u32, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Frame)?;
        let frame_lock = object.frame().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Frame,
        })?;
        let capacity = object.frame_capacity();
        let mut capacity_guard = capacity.map(|capacity| capacity.lock());
        let mut slot = frame_lock.lock();
        let frame = slot.as_mut().ok_or(KernelError::Hmp(HmpError::NotMapped))?;
        // 固定驻留的 Frame 必须先解除 pin；直接释放会把计数降到 0 又归还失败。
        if frame.is_pinned() {
            return Err(KernelError::Hmp(HmpError::PermissionDenied));
        }
        let refs = frame.release();
        if refs > 0 {
            return Ok(refs);
        }
        if let Some(capacity) = capacity_guard.as_mut() {
            capacity.free(&mut slot).map_err(KernelError::Hmp)?;
        } else {
            // 导入的区间没有 Capacity 归属，只丢弃描述。
            slot.take();
        }
        Ok(0)
    }

    /// 设置 Frame 的固定驻留状态。
    pub fn pin_frame(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Frame)?;
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
        capacity.free(&mut slot).map_err(KernelError::Hmp)
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

    /// 返回 Capacity 的峰值用量与软上限。
    pub fn capacity_stats(&self, thread: TcbId, handle: Handle) -> Result<(u64, u64), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let capacity = capacity.lock();
        Ok((capacity.peak(), capacity.soft_limit()))
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
        Ok((
            capacity.range(),
            capacity.kind(),
            capacity.page_size(),
            capacity.limit(),
            capacity.used(),
        ))
    }

    /// 从 Capacity 尾部切出子 Capacity 并返回句柄。
    pub fn split_capacity(
        &self,
        thread: TcbId,
        handle: Handle,
        len: usize,
        kind: CapacityKind,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let child = capacity
            .lock()
            .split(CapacityId::next(), len, kind)
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
        let child_capacity = child_lock.lock();
        parent_lock
            .lock()
            .merge(&child_capacity)
            .map_err(KernelError::Hmp)
    }

    /// 从 Capacity 分配 Frame 并返回 Frame 句柄。
    pub fn allocate_frame(
        &self,
        thread: TcbId,
        handle: Handle,
        len: usize,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::WRITE, ObjectKind::Capacity)?;
        let capacity = object.capacity().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Capacity,
        })?;
        let frame = capacity.lock().allocate(len).map_err(KernelError::Hmp)?;
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
        let mut ledger = Ledger::create(capability.size());
        if let Some(backing) = &backing {
            let _ = ledger.charge(backing.range().len());
        }
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Capability {
                capability,
                backing,
                provider: Mutex::new(None),
                pages: Mutex::new(BTreeMap::new()),
                ledger: Mutex::new(ledger),
                pinned: AtomicBool::new(false),
                scanout: AtomicBool::new(false),
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
        let (object, _) =
            self.resolve(thread, capability, Rights::ADMIN, ObjectKind::Capability)?;
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

    /// 设置类型化能力的固定驻留状态；scanout 驻留期间不能解除固定。
    pub fn pin_capability(
        &self,
        thread: TcbId,
        handle: Handle,
        pinned: bool,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Capability)?;
        if object.capability().is_none() {
            return Err(KernelError::WrongKind {
                expected: ObjectKind::Capability,
            });
        }
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
        object.set_capability_pinned(enabled);
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
