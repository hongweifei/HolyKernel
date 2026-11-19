//! 类型化能力的创建与物理区间查询。

use hk_hal::{PAGE_SIZE, PhysAddr};
use hk_handle::{ObjectId, Rights};
use hk_mem::hmp::{
    Capability, CapabilityKind, CapabilityOp, FrameBacking, OwnedFrame, ProviderKind,
};

use crate::runtime::{Pending, Switch, UserPlatform, UserRuntime};
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::Handle;
use hk_ipc::Message;
use hk_kernel::TcbId;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 常驻能力首次可读时必须为零。物理页只保证归还时清零，直接分配路径
    /// 需在此显式清零，避免把上一使用者的内容暴露给新能力。
    fn zero_backing(&self, backing: &FrameBacking) -> bool {
        let FrameBacking::Owned(_) = backing else {
            // 导入区间是设备/Memory Server 提供的既有内容，不在分配路径清零。
            return true;
        };
        let range = backing.range();
        if self.platform.zero_phys(range.start(), range.len() as usize) {
            return true;
        }
        let zero = [0_u8; PAGE_SIZE];
        let mut offset = 0_u64;
        while offset < range.len() {
            let phys = PhysAddr::new(range.start().get().saturating_add(offset));
            if !self.platform.write_phys(phys, &zero) {
                return false;
            }
            offset = offset.saturating_add(PAGE_SIZE as u64);
        }
        true
    }

    pub(super) fn allocate_owned(
        &mut self,
        index: usize,
        len: u64,
        capacity_number: Option<usize>,
    ) -> Option<FrameBacking> {
        // Capacity 句柄登记在进程主线程的句柄表；同进程线程共享该容量，
        // 因此必须用主线程的 TCB 解析，线程自己的句柄表没有这份句柄。
        let tcb = self.tasks[self.leader_of(index)].tcb;
        let handle = match capacity_number {
            Some(number) => self.handle_at(index, number)?,
            None => self.process(index).allocation_capacity,
        };
        let capacity = self.kernel.capacity_ref_write(tcb, handle).ok()?;
        let allocated = { capacity.lock().allocate(len as usize) };
        let frame = match allocated {
            Ok(frame) => frame,
            Err(_) => {
                if self.reclaim_anonymous_pages(index, len) == 0 {
                    return None;
                }
                capacity.lock().allocate(len as usize).ok()?
            }
        };
        Some(FrameBacking::Owned(OwnedFrame::new(frame, capacity)))
    }

    /// 缺页/镜像装入路径的分配：失败时进入统一 OOM 策略并重试一次。
    pub(super) fn allocate_owned_with_oom(
        &mut self,
        cpu: usize,
        index: usize,
        len: u64,
        capacity_number: Option<usize>,
    ) -> Option<FrameBacking> {
        if let Some(backing) = self.allocate_owned(index, len, capacity_number) {
            return Some(backing);
        }
        if !self.oom_guard(cpu, index, len) {
            return None;
        }
        self.allocate_owned(index, len, capacity_number)
    }

    pub(crate) fn capability_create(
        &mut self,
        cpu: usize,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        self.capability_create_with(cpu, index, tcb, frame, None)
    }

    pub(crate) fn capability_create_on(
        &mut self,
        cpu: usize,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let capacity_number = (frame.arg2() != 0).then_some(frame.arg2() as usize);
        self.capability_create_with(cpu, index, tcb, frame, capacity_number)
    }

    fn capability_create_with(
        &mut self,
        cpu: usize,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
        capacity_number: Option<usize>,
    ) -> Switch {
        let Some(kind) = CapabilityKind::from_u8(frame.arg0() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg1() as usize;
        if length == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        let Ok(charge) = self.charge_frame(index, aligned) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let backing = match self.allocate_owned(index, aligned, capacity_number) {
            Some(backing) => backing,
            None => {
                if !self.oom_guard(cpu, index, aligned) {
                    self.release_frame(charge);
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
                let Some(retry) = self.allocate_owned(index, aligned, capacity_number) else {
                    self.release_frame(charge);
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                retry
            }
        };
        if !self.zero_backing(&backing) {
            self.release_frame(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(capability) = Capability::new(kind, aligned, PAGE_SIZE) else {
            self.release_frame(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let handle = match self.kernel.create_capability(
            tcb,
            capability,
            Some(backing),
            Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::TRANSFER | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                self.release_frame(charge);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_capability(index, handle, Some(charge)));
        Switch::Continue
    }

    pub(crate) fn numa_migrate(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let offset = frame.arg1();
        let len = frame.arg2();
        let node = frame.arg3() as u8;
        let Some((handle, _, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let migrated = self.migrate_capability(index, tcb, handle, offset, len, node);
        frame.set_result(migrated);
        Switch::Continue
    }

    pub(crate) fn capability_swap(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let offset = frame.arg1();
        let length = frame.arg2().max(hk_hal::PAGE_SIZE as u64);
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Swap).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(Some(provider)) = self.kernel.capability_provider(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let page = hk_hal::PAGE_SIZE as u64;
        let start = (offset / page) as usize;
        let end = (offset
            .saturating_add(length)
            .min(capability.size())
            .div_ceil(page)) as usize;
        if start >= end {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(object) = self.kernel.capability_id(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self.kernel.endpoint_served(provider) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        match self.continue_provider_reclaim(index, tcb, handle, object, provider, start, end, 0) {
            Some(released) => {
                frame.set_result(if end - start == 1 && released == 0 {
                    u64::MAX
                } else {
                    released
                });
                Switch::Continue
            }
            None => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                Switch::Block
            }
        }
    }

    /// 把能力区间按连续页批量交给 Provider；返回 `None` 表示已发送请求并阻塞。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn continue_provider_reclaim(
        &mut self,
        index: usize,
        tcb: TcbId,
        handle: Handle,
        object: ObjectId,
        provider: ObjectId,
        mut page_index: usize,
        end: usize,
        released: u64,
    ) -> Option<u64> {
        const BATCH_PAGES: usize = 16;
        if !self.kernel.endpoint_served(provider) {
            return Some(u64::MAX);
        }
        let page = hk_hal::PAGE_SIZE as u64;
        while page_index < end {
            // 找到本批第一页。
            loop {
                if page_index >= end {
                    return Some(released);
                }
                let Ok(Some(_)) = self.kernel.capability_page(tcb, handle, page_index) else {
                    page_index += 1;
                    continue;
                };
                if matches!(self.kernel.capability_pinned(tcb, handle), Ok(true)) {
                    page_index += 1;
                    continue;
                }
                break;
            }
            let first = page_index;
            let mut count = 0;
            let mut data = alloc::vec::Vec::with_capacity(hk_hal::PAGE_SIZE * BATCH_PAGES);
            while page_index < end && count < BATCH_PAGES {
                let Ok(Some(range)) = self.kernel.capability_page(tcb, handle, page_index) else {
                    break;
                };
                if matches!(self.kernel.capability_pinned(tcb, handle), Ok(true)) {
                    break;
                }
                if !matches!(
                    self.kernel
                        .begin_capability_reclaim(tcb, handle, page_index),
                    Ok(true)
                ) {
                    // 同一页已有在途换出：整批失败，避免把重复请求当作
                    // “已跳过”而返回成功。
                    for page in first..page_index {
                        self.kernel.end_capability_reclaim(tcb, handle, page);
                    }
                    return Some(u64::MAX);
                }
                // 先解除所有 PTE，Provider 保存期间不会被并发写入；任何
                // alias 解除失败都必须中止，不能带着仍存活的 PTE 释放物理页。
                let unmapped = self.unmap_capability_page(index, object, page_index);
                if !unmapped {
                    for page in first..=page_index {
                        self.kernel.end_capability_reclaim(tcb, handle, page);
                    }
                    return Some(u64::MAX);
                }
                let mut buffer = [0_u8; hk_hal::PAGE_SIZE];
                if !self.platform.read_phys(range.start(), &mut buffer) {
                    self.kernel.end_capability_reclaim(tcb, handle, page_index);
                    break;
                }
                data.extend_from_slice(&buffer);
                count += 1;
                page_index += 1;
            }
            if count == 0 {
                page_index += 1;
                continue;
            }
            let words = [
                object.get(),
                first as u64,
                first as u64 * page,
                page,
                if count == 1 { 0 } else { count as u64 },
                self.oom.events,
                0,
                0,
            ];
            let label = if count == 1 {
                hk_syscall::PROVIDER_RECLAIM
            } else {
                hk_syscall::PROVIDER_RECLAIM_BATCH
            };
            let message = Message::new(label).with_words(words).with_data(data);
            if self.kernel.call_object(tcb, provider, message).is_ok() {
                let deadline = self.contract_now().saturating_add(1_000_000);
                self.tasks[index].pending = Pending::ProviderReclaim {
                    capability: handle,
                    index: first,
                    object,
                    deadline,
                    provider,
                    end,
                    released,
                    count,
                };
                self.timeouts.entry(deadline).or_default().push(index);
                return None;
            }
            for page in first..first + count {
                self.kernel.end_capability_reclaim(tcb, handle, page);
            }
        }
        Some(released)
    }

    pub(crate) fn capability_pin(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let pinned = frame.arg1() != 0;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Pin).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if self.kernel.pin_capability(tcb, handle, pinned).is_err() {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(0);
        }
        Switch::Continue
    }

    /// 设置 Surface 的 scanout 驻留；开启时同时固定，驻留期间不能解除固定。
    pub(crate) fn capability_scanout(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let enabled = frame.arg1() != 0;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Scanout).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if self
            .kernel
            .scanout_capability(tcb, handle, enabled)
            .is_err()
        {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(0);
        }
        Switch::Continue
    }

    pub(crate) fn capability_commit(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Commit).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        match self.kernel.commit_capability(tcb, handle) {
            Ok(version) => frame.set_result(version),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    /// 请求 Provider 回写能力的常驻页。
    pub(crate) fn capability_writeback(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let number = frame.arg0() as usize;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Writeback).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if !self
            .kernel
            .handle_rights(tcb, handle, hk_handle::ObjectKind::Capability)
            .is_ok_and(|rights| rights.contains(hk_handle::Rights::WRITE))
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(Some(provider)) = self.kernel.capability_provider(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self.kernel.endpoint_served(provider) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(id) = self.kernel.handle_id(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut words = [0_u64; hk_ipc::MAX_MESSAGE_WORDS];
        words[0] = id.get();
        if self
            .kernel
            .post_to_endpoint(
                provider,
                Message::new(hk_syscall::PROVIDER_WRITEBACK).with_words(words),
            )
            .is_err()
        {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(0);
        }
        Switch::Continue
    }

    pub(crate) fn capability_export(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Export).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        match self.kernel.duplicate_handle(tcb, handle, rights) {
            Ok(copy) => {
                // 导出只复制句柄；同一对象的驻留内存已经计入本进程。
                frame.set_result(self.install_extra_handle(index, copy));
            }
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn capability_create_lazy(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let Some(kind) = CapabilityKind::from_u8(frame.arg0() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg1() as usize;
        if length == 0 || kind == CapabilityKind::Frame {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        // 惰性能力在缺页装入物理页时才计入 Task Ledger。
        let Ok(capability) = Capability::new(kind, aligned, PAGE_SIZE) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let handle = match self.kernel.create_capability(
            tcb,
            capability,
            None,
            Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::TRANSFER | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_capability(index, handle, None));
        Switch::Continue
    }

    /// Provider 按对象标识重新取得自己提供的能力句柄。
    pub(crate) fn capability_grant(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let id = ObjectId::new(frame.arg0());
        let rights = Rights::from_bits_truncate(frame.arg1() as u32);
        match self.kernel.grant_capability(tcb, id, rights) {
            Ok(handle) => frame.set_result(self.install_capability(index, handle, None)),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn capability_bind_provider(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let number = frame.arg0() as usize;
        let endpoint = frame.arg1() as usize;
        let Some(kind) = ProviderKind::from_u8(frame.arg2() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some((capability, _, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(endpoint) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self
            .kernel
            .bind_capability_provider(tcb, capability, endpoint, kind)
        {
            Ok(()) => frame.set_result(0),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn ledger_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let resident = self.process_resident(index);
        let (charged, limit, peak, pressure, resident, soft_limit) = {
            let ledger = &mut self.process_mut(index).ledger;
            ledger.set_resident(resident);
            (
                ledger.charged(),
                ledger.limit(),
                ledger.peak(),
                ledger.pressure(),
                ledger.resident(),
                ledger.soft_limit(),
            )
        };
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&charged.to_le_bytes());
        bytes[8..16].copy_from_slice(&limit.to_le_bytes());
        bytes[16..24].copy_from_slice(&peak.to_le_bytes());
        bytes[24..32].copy_from_slice(&pressure.to_le_bytes());
        bytes[32..40].copy_from_slice(&resident.to_le_bytes());
        bytes[40..].copy_from_slice(&soft_limit.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn capability_phys(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some((_, capability, Some(range))) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Phys).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if capacity < 16 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..].copy_from_slice(&range.len().to_le_bytes());
        if self.platform.write_user(out, &bytes) {
            frame.set_result(16);
        } else {
            frame.set_result(u64::MAX);
        }
        Switch::Continue
    }
}
