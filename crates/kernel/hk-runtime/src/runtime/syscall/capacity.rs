//! Capacity 与 Frame 对象操作。

use hk_hal::VirtAddr;
use hk_handle::Rights;
use hk_mem::hmp::{
    Capability, CapabilityKind, CapacityKind, FrameBacking, OwnedFrame, Region, RegionFlags,
};
use hk_syscall::{
    CAPACITY_KIND_DMA, CAPACITY_KIND_GPU, CAPACITY_KIND_NUMA, CAPACITY_KIND_SYSTEM,
    CAPACITY_KIND_USER,
};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn gpu_capacity(&mut self, index: usize, frame: &mut C) -> Switch {
        if self.tasks[index].name != "hk-server-virtio" {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(capacity) = self.platform.gpu_capacity() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let tcb = self.tasks[index].tcb;
        // Gpu 资源与设备 Dma 共用同一本 Task Ledger；授予 Gpu 时同步放宽
        // 配额，否则 1 MiB 的默认 Dma 上限会拒绝扫描输出大缓冲。
        let ledger_limit = self.process(index).ledger.limit();
        self.process_mut(index)
            .ledger
            .set_limit(ledger_limit.saturating_add(capacity.lock().limit()));
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let Ok(handle) = self.kernel.create_capacity_shared(tcb, capacity, rights) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        frame.set_result(self.install_capacity(index, handle));
        Switch::Continue
    }

    pub(crate) fn capacity_soft_limit(&mut self, index: usize, frame: &mut C) -> Switch {
        if !matches!(self.tasks[index].name.as_str(), "root" | "hk-server-tempo") {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(handle) = self.handle_at(index, frame.arg0() as usize) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let tcb = self.tasks[index].tcb;
        let Ok(capacity) = self.kernel.capacity_ref(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        capacity.lock().set_soft_limit(frame.arg1());
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn capacity_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, kind, page_size, limit, used)) =
            self.kernel.capacity_info(self.tasks[index].tcb, handle)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((peak, soft_limit)) = self.kernel.capacity_stats(self.tasks[index].tcb, handle)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 72 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(node) = self.kernel.capacity_node(self.tasks[index].tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; 72];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..16].copy_from_slice(&range.len().to_le_bytes());
        bytes[16..24].copy_from_slice(&(capacity_kind_code(kind) as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(page_size as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&limit.to_le_bytes());
        bytes[40..48].copy_from_slice(&used.to_le_bytes());
        bytes[48..56].copy_from_slice(&peak.to_le_bytes());
        bytes[56..64].copy_from_slice(&soft_limit.to_le_bytes());
        let node = node.map_or(u32::MAX, u32::from);
        bytes[64..68].copy_from_slice(&node.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            72
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    /// 枚举 Capacity 持有的全部物理窗口。
    ///
    /// `capacity == 0` 时只返回窗口数；否则按 16 字节记录写入调用方缓冲。
    pub(crate) fn capacity_windows(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let buffer = frame.arg2() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(windows) = self.kernel.capacity_windows(self.tasks[index].tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if buffer == 0 {
            frame.set_result(windows.len() as u64);
            return Switch::Continue;
        }
        let needed = windows
            .len()
            .saturating_mul(hk_syscall::CAPACITY_WINDOW_BYTES);
        if buffer < needed {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut offset = 0usize;
        for window in &windows {
            let Some(address) = out.checked_add(offset as u64) else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            let mut bytes = [0_u8; hk_syscall::CAPACITY_WINDOW_BYTES];
            bytes[..8].copy_from_slice(&window.start().get().to_le_bytes());
            bytes[8..].copy_from_slice(&window.len().to_le_bytes());
            if !self.platform.write_user(address, &bytes) {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
            offset += hk_syscall::CAPACITY_WINDOW_BYTES;
        }
        frame.set_result(needed as u64);
        Switch::Continue
    }

    pub(crate) fn capacity_split(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let length = frame.arg1() as usize;
        let page_size = frame.arg3() as usize;
        let node = ((frame.arg2() >> 8) & 0xFF) as u8;
        let Some(kind) = capacity_kind_from_code(frame.arg2() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let kind = match kind {
            CapacityKind::Numa(_) => CapacityKind::Numa(node),
            other => other,
        };
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let node = matches!(kind, CapacityKind::Numa(_)).then_some(node);
        let Ok(child) = self
            .kernel
            .split_capacity(tcb, handle, length, kind, page_size, node)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        frame.set_result(self.install_capacity(index, child));
        Switch::Continue
    }

    /// 把一个已清空的子 Capacity 并回父 Capacity，并关闭子句柄。
    pub(crate) fn capacity_merge(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let parent = frame.arg0() as usize;
        let child = frame.arg1() as usize;
        let Some(parent_handle) = self.handle_at(index, parent) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(child_handle) = self.handle_at(index, child) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if self
            .kernel
            .merge_capacity(tcb, parent_handle, child_handle)
            .is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let _ = self.kernel.close_handle(tcb, child_handle);
        self.clear_handle(index, child);
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn capacity_alloc(
        &mut self,
        cpu: usize,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let number = frame.arg0() as usize;
        let length = frame.arg1() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let hint = frame.arg2();
        let explicit = (hint != 0).then(|| (hint - 1) as u8);
        // 未显式指定节点时按当前 CPU 的 SRAT 邻近域优先，再按 SLIT 距离回退，
        // 而不是把 None 直接交给后端按物理地址任意挑选。
        let preferred = explicit.or_else(|| {
            self.platform
                .cpu_topology(cpu as u32)
                .map(|topology| topology.proximity.min(u8::MAX as u32) as u8)
        });
        let mut allocated = None;
        if let Some(preferred) = preferred {
            let count = self.platform.numa_node_count().max(1);
            let mut nodes: alloc::vec::Vec<(u8, u8)> = (0..count)
                .map(|node| (self.platform.numa_distance(preferred, node), node))
                .collect();
            nodes.sort_unstable();
            for (_, node) in nodes {
                if let Ok(handle) = self.kernel.allocate_frame(tcb, handle, length, Some(node)) {
                    allocated = Some(handle);
                    break;
                }
            }
        } else if let Ok(handle) = self.kernel.allocate_frame(tcb, handle, length, None) {
            allocated = Some(handle);
        }
        if allocated.is_none() && self.oom_guard(cpu, index, length as u64) {
            // 物理/配额失败可能与全局压力有关：回收当前任务或按策略终止
            // victim 后重试一次；仍失败则走正常错误返回。
            if let Ok(handle) = self.kernel.allocate_frame(tcb, handle, length, preferred) {
                allocated = Some(handle);
            }
        }
        let Some(frame_handle) = allocated else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, _, _, _, _)) = self.kernel.frame_info(tcb, frame_handle) else {
            let _ = self.kernel.close_handle(tcb, frame_handle);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(charge) = self.charge_frame(index, range.len()) else {
            let _ = self.kernel.free_frame(tcb, handle, frame_handle);
            let _ = self.kernel.close_handle(tcb, frame_handle);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        frame.set_result(self.install_frame(index, frame_handle, Some(charge)));
        Switch::Continue
    }

    pub(crate) fn capacity_free(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let capacity = frame.arg0() as usize;
        let frame_number = frame.arg1() as usize;
        frame.set_result(self.free_frame_at(index, tcb, capacity, frame_number));
        Switch::Continue
    }

    pub(crate) fn frame_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, page_size, capacity_id, refs, pinned)) =
            self.kernel.frame_info(self.tasks[index].tcb, handle)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..16].copy_from_slice(&range.len().to_le_bytes());
        bytes[16..24].copy_from_slice(&(page_size as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&capacity_id.get().to_le_bytes());
        bytes[32..40].copy_from_slice(&(refs as u64).to_le_bytes());
        bytes[40..].copy_from_slice(&u64::from(pinned).to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    /// 把用户缓冲写入 Frame 的常驻内容。
    pub(crate) fn frame_write(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        self.frame_content(index, tcb, frame, true)
    }

    /// 把 Frame 的常驻内容读入用户缓冲。
    pub(crate) fn frame_read(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        self.frame_content(index, tcb, frame, false)
    }

    fn frame_content(&mut self, index: usize, tcb: TcbId, frame: &mut C, write: bool) -> Switch {
        let number = frame.arg0() as usize;
        let offset = frame.arg1();
        let address = frame.arg2();
        let length = frame.arg3() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(rights) = self
            .kernel
            .handle_rights(tcb, handle, hk_handle::ObjectKind::Frame)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let required = if write { Rights::WRITE } else { Rights::READ };
        if !rights.contains(required) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok((range, _, _, _, _)) = self.kernel.frame_info(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(end) = offset.checked_add(length as u64) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if end > range.len() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut done = 0;
        let mut buffer = [0_u8; 4096];
        while done < length {
            let chunk = (length - done).min(buffer.len());
            let phys = hk_hal::PhysAddr::new(range.start().get() + offset + done as u64);
            let copied = if write {
                self.platform
                    .read_user(address + done as u64, &mut buffer[..chunk])
                    && self.platform.write_phys(phys, &buffer[..chunk])
            } else {
                self.platform.read_phys(phys, &mut buffer[..chunk])
                    && self
                        .platform
                        .write_user(address + done as u64, &buffer[..chunk])
            };
            if !copied {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
            done += chunk;
        }
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn frame_pin(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let pinned = frame.arg1() != 0;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self.kernel.pin_frame(tcb, handle, pinned) {
            Ok(()) => frame.set_result(0),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn frame_map(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let packed = frame.arg1();
        let Some(frame_handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let rights = Rights::from_bits_truncate(packed as u32);
        let flags = RegionFlags::from_bits_truncate((packed >> 32) as u32);
        let Ok(granted) =
            self.kernel
                .handle_rights(tcb, frame_handle, hk_handle::ObjectKind::Frame)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !granted.covers(rights) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        // 先按目标能力几何做一次 Region 校验再消费句柄：失败的系统调用不能
        // 销毁调用方的 Frame（会导致数据页被清零且无法重试）。GUARD 要求页面
        // 永不安装映射，与 Frame 的常驻 backing 互斥。
        let Ok((range, page_size, _, _, _)) = self.kernel.frame_info(tcb, frame_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(probe) = Capability::new(CapabilityKind::Frame, range.len(), page_size) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if flags.contains(RegionFlags::GUARD)
            || Region::new(&probe, 0, range.len() as usize, rights, flags).is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(owned) = self.kernel.take_owned_frame(tcb, frame_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let result = self.finish_frame_map(index, tcb, number, owned, rights, flags);
        let _ = self.kernel.close_handle(tcb, frame_handle);
        self.clear_handle(index, number);
        frame.set_result(result);
        Switch::Continue
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_frame_map(
        &mut self,
        index: usize,
        tcb: TcbId,
        number: usize,
        owned: OwnedFrame,
        rights: Rights,
        flags: RegionFlags,
    ) -> u64 {
        let range = owned.range();
        let length = owned.len();
        let page_size = owned.page_size();
        if length == 0 || page_size == 0 || !length.is_multiple_of(page_size) {
            return u64::MAX;
        }
        let Ok(capability) = Capability::new(CapabilityKind::Frame, range.len(), page_size) else {
            return u64::MAX;
        };
        let backing = FrameBacking::Owned(owned);
        let Ok(capability_handle) = self.kernel.create_capability(
            tcb,
            capability.clone(),
            Some(backing),
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        ) else {
            return u64::MAX;
        };
        let cleanup = |runtime: &mut Self| {
            let _ = runtime.kernel.close_handle(tcb, capability_handle);
        };
        // `PINNED` 在帧映射上与 Region 映射语义一致：固定代建的能力，阻止回收。
        if flags.contains(RegionFlags::PINNED)
            && self
                .kernel
                .pin_capability_internal(tcb, capability_handle, true)
                .is_err()
        {
            cleanup(self);
            return u64::MAX;
        }
        let Ok(mut region) = Region::new(&capability, 0, length, rights, flags) else {
            cleanup(self);
            return u64::MAX;
        };
        let Some(virt) = self.process_mut(index).user_window.allocate(length) else {
            cleanup(self);
            return u64::MAX;
        };
        if !self.platform.map_region(
            index,
            &hk_mem::RegionMapping {
                virt,
                phys: range.start().get(),
                len: length,
                rights,
                flags,
                policy: region.cache(),
            },
        ) {
            self.process_mut(index).user_window.release(virt);
            cleanup(self);
            return u64::MAX;
        }
        if region.map(VirtAddr::new(virt)).is_err() {
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
            cleanup(self);
            return u64::MAX;
        }
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
            cleanup(self);
            return u64::MAX;
        }
        let Ok(region_handle) =
            self.kernel
                .create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN)
        else {
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
            cleanup(self);
            return u64::MAX;
        };
        let charge = self.take_frame_charge(index, number);
        self.install_region(
            index,
            region_handle,
            capability_handle,
            region_id,
            true,
            virt,
            length,
            0,
            rights,
            flags,
            charge,
        )
    }

    fn free_frame_at(
        &mut self,
        index: usize,
        tcb: TcbId,
        capacity: usize,
        frame_number: usize,
    ) -> u64 {
        let Some(capacity) = self.handle_at(index, capacity) else {
            return u64::MAX;
        };
        let Some(frame) = self.handle_at(index, frame_number) else {
            return u64::MAX;
        };
        if self.kernel.free_frame(tcb, capacity, frame).is_err() {
            return u64::MAX;
        }
        let _ = self.kernel.close_handle(tcb, frame);
        self.clear_handle(index, frame_number);
        1
    }
}

fn capacity_kind_code(kind: CapacityKind) -> u8 {
    match kind {
        CapacityKind::System => CAPACITY_KIND_SYSTEM,
        CapacityKind::User => CAPACITY_KIND_USER,
        CapacityKind::Dma => CAPACITY_KIND_DMA,
        CapacityKind::Gpu => CAPACITY_KIND_GPU,
        CapacityKind::Numa(_) => CAPACITY_KIND_NUMA,
    }
}

fn capacity_kind_from_code(code: u8) -> Option<CapacityKind> {
    Some(match code {
        CAPACITY_KIND_SYSTEM => CapacityKind::System,
        CAPACITY_KIND_USER => CapacityKind::User,
        CAPACITY_KIND_DMA => CapacityKind::Dma,
        CAPACITY_KIND_GPU => CapacityKind::Gpu,
        CAPACITY_KIND_NUMA => CapacityKind::Numa(0),
        _ => return None,
    })
}
