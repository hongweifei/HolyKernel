//! 调度、退出与阻塞唤醒。

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use hk_handle::{Handle, ObjectId};
use hk_kernel::TcbId;
use hk_syscall::CONSOLE_NOTIFY_INPUT;

use crate::runtime::{Pending, Step, UserPlatform, UserRuntime};

/// Provider 回复无效时的退出码（ASCII "PROV"）。
const PROVIDER_FAULT_EXIT: u64 = 0x5052_4f56;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    fn drain_irqs(&mut self) {
        let mut pending = [0_u64; 4];
        self.platform.take_irqs(&mut pending);
        for (word, bits) in pending.iter().enumerate() {
            if *bits == 0 {
                continue;
            }
            for bit in 0..64 {
                if bits & (1_u64 << bit) != 0 {
                    self.signal_irq((word * 64 + bit) as u8);
                }
            }
        }
    }

    pub(crate) fn release_grants(&mut self, index: usize) {
        let tcb = self.tasks[index].tcb;
        self.release_all_dma_mappings(index);
        for entry in core::mem::take(&mut self.tasks[index].grants)
            .into_iter()
            .flatten()
        {
            let handle = entry.handle;
            self.release_bindings(index, handle);
            if let Ok(grant) = self.kernel.device_grant(tcb, handle)
                && let Some(range) = grant.dma
            {
                self.platform.dma_free(range);
            }
            if let Some(charge) = entry.charge {
                self.release_frame(charge);
            }
        }
    }

    pub(crate) fn signal_irq(&self, irq: u8) {
        for task in &self.tasks {
            for (_, line, notification, bits) in &task.bound_irqs {
                if *line == irq {
                    let _ = self
                        .kernel
                        .signal_id(ObjectId::new(notification.get()), *bits);
                }
            }
        }
    }

    pub(crate) fn release_dma_mappings(&mut self, index: usize, grant: Handle) {
        let tcb = self.tasks[index].tcb;
        let mappings = core::mem::take(&mut self.tasks[index].dma_mappings);
        let mut kept = Vec::new();
        for (handle, object, domain, iova, len, kind) in mappings {
            if handle == grant {
                if let Some(iommu) = self.platform.iommu() {
                    let _ = iommu.unmap(domain, iova, len);
                }
                let _ = self.unpin_dma_object(tcb, object, kind);
            } else {
                kept.push((handle, object, domain, iova, len, kind));
            }
        }
        self.tasks[index].dma_mappings = kept;
    }

    fn release_all_dma_mappings(&mut self, index: usize) {
        let tcb = self.tasks[index].tcb;
        let mappings = core::mem::take(&mut self.tasks[index].dma_mappings);
        for (_, object, domain, iova, len, kind) in mappings {
            if let Some(iommu) = self.platform.iommu() {
                let _ = iommu.unmap(domain, iova, len);
            }
            let _ = self.unpin_dma_object(tcb, object, kind);
        }
    }

    pub(crate) fn release_bindings(&mut self, index: usize, grant: Handle) {
        let bindings = core::mem::take(&mut self.tasks[index].bound_irqs);
        for (handle, irq, notification, bits) in bindings {
            if handle == grant {
                self.release_irq(irq);
            } else {
                self.tasks[index]
                    .bound_irqs
                    .push((handle, irq, notification, bits));
            }
        }
    }

    pub(crate) fn release_irq(&mut self, irq: u8) {
        let count = &mut self.irq_routes[irq as usize];
        if *count > 0 {
            *count -= 1;
            if *count == 0
                && let Some(controller) = self.platform.interrupt_controller()
            {
                let _ = controller.unbind(hk_hal::Irq::new(u32::from(irq)));
            }
        }
    }
    /// 调度一步；调用方在锁外恢复 `context`，再调用 [`Self::finish`]。
    pub fn step(&mut self, cpu: usize) -> Step<C> {
        self.kernel.set_clock(self.contract_now());
        self.drain_irqs();
        self.wake_sleepers();
        self.wake_timeouts();
        if self.platform.take_console_input()
            && let Some(index) = self.console_subscriber
            && let Some(task) = self.tasks.get(index)
            && !task.exited
            && let Some(handle) = task.notifications.get(index).copied().flatten()
        {
            let _ = self.kernel.signal(task.tcb, handle, CONSOLE_NOTIFY_INPUT);
        }
        let Some(index) = self.next_task(cpu) else {
            self.last_finish = None;
            return Step::Done;
        };

        if self.prepare(cpu, index) {
            return Step::Blocked;
        }

        if let Some(last) = self.last_finish.take() {
            let now = self.platform.timestamp();
            if now >= last {
                let cycles = now - last;
                self.switch_count = self.switch_count.saturating_add(1);
                self.switch_cycles = self.switch_cycles.saturating_add(cycles);
                self.switch_max = self.switch_max.max(cycles);
            }
        }

        self.platform.set_thread_pointer(self.tasks[index].tls);
        self.resumes[cpu] = self.resumes[cpu].saturating_add(1);
        self.run_start[cpu] = self.platform.timestamp();
        Step::Resume {
            index,
            context: self.tasks[index].context,
        }
    }

    /// 恢复执行返回后收尾；任务退出时关闭其他任务持有的该端点句柄，并返回名称与退出码。
    pub fn finish(&mut self, cpu: usize, index: usize) -> Option<(String, u64)> {
        self.current[cpu] = None;
        let now = self.platform.timestamp();
        let started = self.run_start[cpu];
        if now >= started {
            let cycles = now - started;
            self.tasks[index].cpu_cycles = self.tasks[index].cpu_cycles.saturating_add(cycles);
            self.cpu_busy[cpu] = self.cpu_busy[cpu].saturating_add(cycles);
            let elapsed = self.contract_ticks(cycles);
            self.kernel.charge(self.tasks[index].tcb, elapsed);
        }
        self.last_finish = Some(now);
        if !self.tasks[index].exited {
            return None;
        }

        self.remove_sleep(index);
        let exited_tcb = self.tasks[index].tcb;
        self.wake_joiners(exited_tcb);
        if self.tasks[index].leader != index {
            return None;
        }

        let exited_endpoint = index as u64 + 1;
        for other in 0..self.tasks.len() {
            if other == index || self.tasks[other].exited {
                continue;
            }
            let tcb = self.tasks[other].tcb;
            match self.tasks[other].pending {
                Pending::RecvData {
                    handle,
                    expected: Some(expected),
                    ..
                } if expected == exited_tcb => {
                    let _ = self.kernel.cancel_recv(tcb, handle);
                    let mut context = self.tasks[other].context;
                    // 只等待该实例消息的接收者不会再有输入；返回错误让调用方
                    // 区分「服务已退出」与「暂时没有事件」。
                    context.set_result(u64::MAX);
                    context.set_handle(0);
                    context.set_sender(0);
                    self.tasks[other].context = context;
                    self.tasks[other].pending = Pending::None;
                    self.kernel.wake_thread(tcb);
                }
                Pending::Send { endpoint, handle } if endpoint == exited_endpoint => {
                    let _ = self.kernel.cancel_send(tcb, handle);
                    let mut context = self.tasks[other].context;
                    context.set_result(u64::MAX);
                    self.tasks[other].context = context;
                    self.tasks[other].pending = Pending::None;
                    self.kernel.wake_thread(tcb);
                }
                _ => {}
            }
        }

        for other in 0..self.tasks.len() {
            if other == index || self.tasks[other].exited {
                continue;
            }
            if let Some(handle) = self.tasks[other].handles.get(index).copied().flatten() {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].handles[index] = None;
            }
            if let Some(handle) = self.tasks[other]
                .notifications
                .get(index)
                .copied()
                .flatten()
            {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].notifications[index] = None;
            }
        }

        let task = &self.tasks[index];
        Some((task.name.clone(), task.exit_code))
    }

    /// 命中调度 tick 时保存现场；返回是否切换任务。
    pub fn preempt(&mut self, cpu: usize, context: C) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        self.tasks[index].context = context;
        self.tasks[index].pending = Pending::None;
        true
    }

    /// 终止一个未在任何 CPU 上执行的任务；成功返回 `true`。
    pub fn terminate(&mut self, name: &str, code: u64) -> bool {
        let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.name == name && !task.exited)
        else {
            return false;
        };
        if self
            .current
            .iter()
            .flatten()
            .any(|current| *current == index)
        {
            return false;
        }

        let tcb = self.tasks[index].tcb;
        self.migrate_thread_regions(index);
        self.release_grants(index);
        self.release_task_charges(index);
        if self.tasks[index].leader == index {
            self.teardown_process(index);
        }
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.remove_sleep(index);
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
        self.platform.release_pages(index);
        self.recover_dead_provider();
        true
    }

    /// 端点服务退出后，处理等待该 Provider 的阻塞任务。
    pub(crate) fn recover_dead_provider(&mut self) {
        let mut faults = alloc::vec::Vec::new();
        let mut reclaims = alloc::vec::Vec::new();
        for slot in 0..self.tasks.len() {
            if self.tasks[slot].exited {
                continue;
            }
            let tcb = self.tasks[slot].tcb;
            match self.tasks[slot].pending {
                Pending::ProviderFault { capability, .. } => {
                    let dead =
                        self.kernel
                            .capability_provider(tcb, capability)
                            .map_or(true, |provider| {
                                provider
                                    .is_none_or(|endpoint| !self.kernel.endpoint_served(endpoint))
                            });
                    if dead {
                        faults.push(self.tasks[slot].leader);
                    }
                }
                Pending::ProviderReclaim {
                    capability,
                    index,
                    count,
                    ..
                } => {
                    let dead =
                        self.kernel
                            .capability_provider(tcb, capability)
                            .map_or(true, |provider| {
                                provider
                                    .is_none_or(|endpoint| !self.kernel.endpoint_served(endpoint))
                            });
                    if dead {
                        reclaims.push((slot, capability, index, count));
                    }
                }
                _ => {}
            }
        }
        for slot in faults {
            let name = self.tasks[slot].name.clone();
            let _ = self.terminate(&name, PROVIDER_FAULT_EXIT);
        }
        for (slot, capability, page_index, count) in reclaims {
            let tcb = self.tasks[slot].tcb;
            let _ = self.kernel.cancel_call(tcb);
            for page in page_index..page_index + count {
                self.kernel.end_capability_reclaim(tcb, capability, page);
            }
            self.tasks[slot].context.set_result(u64::MAX);
            self.tasks[slot].pending = Pending::None;
            self.kernel.wake_thread(tcb);
        }
    }

    /// 标记当前任务因用户态异常退出；成功返回 `true`。
    pub fn fault(&mut self, cpu: usize, code: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        if self.tasks[index].exited {
            return false;
        }

        let tcb = self.tasks[index].tcb;
        self.migrate_thread_regions(index);
        self.release_grants(index);
        self.release_task_charges(index);
        if self.tasks[index].leader == index {
            self.teardown_process(index);
        }
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.current[cpu] = None;
        self.remove_sleep(index);
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
        self.platform.release_pages(index);
        self.recover_dead_provider();
        true
    }

    /// 唤醒到期的休眠任务。
    /// 登记休眠；同一任务只保留一个 deadline。
    pub(crate) fn add_sleep(&mut self, index: usize, wake: u64) {
        let entries = self.sleeping.entry(wake).or_default();
        if !entries.contains(&index) {
            entries.push(index);
        }
    }

    /// 从 deadline 表移除任务。
    pub(crate) fn remove_sleep(&mut self, index: usize) {
        self.sleeping.retain(|_, entries| {
            entries.retain(|slot| *slot != index);
            !entries.is_empty()
        });
        self.timeouts.retain(|_, entries| {
            entries.retain(|slot| *slot != index);
            !entries.is_empty()
        });
    }

    fn wake_sleepers(&mut self) {
        let now = self.contract_now();
        while let Some((&deadline, _)) = self.sleeping.first_key_value() {
            if deadline > now {
                break;
            }
            let Some(entries) = self.sleeping.remove(&deadline) else {
                break;
            };
            for index in entries {
                let Some(task) = self.tasks.get(index) else {
                    continue;
                };
                if task.exited {
                    continue;
                }
                match task.pending {
                    Pending::Sleep { wake } if now >= wake => {
                        self.kernel.wake_thread(task.tcb);
                    }
                    Pending::Sleep { wake } => self.add_sleep(index, wake),
                    _ => {}
                }
            }
        }
    }

    fn wake_timeouts(&mut self) {
        let now = self.contract_now();
        while let Some((&deadline, _)) = self.timeouts.first_key_value() {
            if deadline > now {
                break;
            }
            let Some(entries) = self.timeouts.remove(&deadline) else {
                break;
            };
            for index in entries {
                let Some((tcb, pending, exited)) = self
                    .tasks
                    .get(index)
                    .map(|task| (task.tcb, task.pending, task.exited))
                else {
                    continue;
                };
                if exited {
                    continue;
                }
                match pending {
                    Pending::RecvData {
                        handle,
                        deadline: Some(at),
                        ..
                    } if at <= now => {
                        // `Ok(false)` 表示发送方已认领该接收等待（消息正在交付），
                        // 或消息已进入待取队列：不按超时处理，交给 prepare 消费。
                        match self.kernel.cancel_recv(tcb, handle) {
                            Ok(true) | Err(_) => {
                                self.tasks[index].context.set_result(u64::MAX);
                                self.tasks[index].pending = Pending::None;
                            }
                            Ok(false) => {}
                        }
                        self.kernel.wake_thread(tcb);
                    }
                    Pending::RecvData {
                        deadline: Some(at), ..
                    } => {
                        self.timeouts.entry(at).or_default().push(index);
                    }
                    Pending::Call {
                        deadline: Some(at), ..
                    } if at <= now => {
                        // 回复已到达时不要丢弃：交给 prepare 取回结果。
                        if self.kernel.cancel_call(tcb) {
                            self.tasks[index].context.set_result(u64::MAX);
                            self.tasks[index].pending = Pending::None;
                        }
                        self.kernel.wake_thread(tcb);
                    }
                    Pending::Call {
                        deadline: Some(at), ..
                    } => {
                        self.timeouts.entry(at).or_default().push(index);
                    }
                    Pending::ProviderReclaim {
                        capability,
                        index: page_index,
                        deadline,
                        count,
                        ..
                    } if deadline <= now => {
                        // 回复已到达时不丢弃：交给 prepare 消费结果。
                        if self.kernel.cancel_call(tcb) {
                            for page in page_index..page_index + count {
                                self.kernel.end_capability_reclaim(tcb, capability, page);
                            }
                            self.tasks[index].context.set_result(u64::MAX);
                            self.tasks[index].pending = Pending::None;
                        }
                        self.kernel.wake_thread(tcb);
                    }
                    Pending::ProviderReclaim { deadline, .. } => {
                        self.timeouts.entry(deadline).or_default().push(index);
                    }
                    Pending::ProviderFault { deadline, .. } => {
                        // deadline 到期后唤醒；prepare 取不到回包会按
                        // PROVIDER_FAULT_EXIT 终止任务。
                        if deadline <= now {
                            self.kernel.wake_thread(tcb);
                        } else {
                            self.timeouts.entry(deadline).or_default().push(index);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn next_task(&mut self, cpu: usize) -> Option<usize> {
        let tcb = self.kernel.schedule(cpu)?;
        self.tasks.iter().position(|task| task.tcb == tcb)
    }

    /// 返回进程分配 COW 私有页所用的 Capacity。
    pub(crate) fn task_capacity(
        &self,
        index: usize,
    ) -> Option<alloc::sync::Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
        let owner = self.leader_of(index);
        let tcb = self.tasks[owner].tcb;
        let handle = self.process(owner).allocation_capacity;
        self.kernel.capacity_ref(tcb, handle).ok()
    }

    /// 返回当前契约时钟（微秒）。平台没有 TSC 频率时按调度 tick 的 10 ms 折算。
    pub(crate) fn contract_now(&self) -> u64 {
        let frequency = self.platform.tsc_frequency();
        if frequency == 0 {
            return self.platform.timer_ticks().saturating_mul(10_000);
        }
        self.contract_ticks(self.platform.timestamp())
    }

    /// 计算同类 Task 的平均虚拟进度，用于恢复时重定基。
    pub(super) fn average_virtual_pass(&self, class: hk_cadence::CadenceClass) -> u64 {
        let mut total = 0_u64;
        let mut count = 0_u64;
        for (index, task) in self.tasks.iter().enumerate() {
            // 线程共享 Task 的 Meter；只按主线程统计一次。
            if task.exited || task.leader != index {
                continue;
            }
            let Some(cadence) = self.kernel.task_cadence_snapshot(task.tcb) else {
                continue;
            };
            if cadence.class != class {
                continue;
            }
            if let Some(meter) = self.kernel.task_meter_snapshot(task.tcb) {
                total = total.saturating_add(meter.virtual_pass);
                count += 1;
            }
        }
        total.checked_div(count).unwrap_or(0)
    }

    /// 返回 `cpu` 下一次需要唤醒的契约时刻：本地 Grant 结束、预算补充或 sleep 到期。
    pub fn next_wakeup(&self, cpu: usize) -> Option<u64> {
        let mut earliest = self.kernel.next_deadline(cpu);
        if let Some((&deadline, _)) = self.sleeping.first_key_value() {
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        if let Some((&deadline, _)) = self.timeouts.first_key_value() {
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        earliest
    }

    /// 把 `cpu` 的下一次唤醒写给平台；没有待办时只保留 watchdog。
    pub fn arm_wakeup(&self, cpu: usize) {
        self.platform.arm_wakeup(self.next_wakeup(cpu));
    }

    /// 把进程账本挂到内核 Task，供句柄转移在核内记账。
    pub(super) fn attach_task_ledger(&self, tcb: TcbId, ledger: &hk_mem::hmp::SharedLedger) {
        if let Some(task) = self.kernel.task_of(tcb) {
            let _ = self.kernel.set_task_ledger(task, ledger.clone());
        }
    }

    /// 把平台周期数换算为契约 tick。
    fn contract_ticks(&self, cycles: u64) -> u64 {
        let frequency = self.platform.tsc_frequency();
        if frequency == 0 {
            return 0;
        }
        let micros =
            (u128::from(cycles) * 1_000_000 / u128::from(frequency)).min(u128::from(u64::MAX));
        micros as u64
    }

    fn prepare(&mut self, cpu: usize, index: usize) -> bool {
        self.current[cpu] = Some(index);
        // 恢复阻塞接收前先切到目标地址空间，字节载荷才能写入用户缓冲。
        self.platform.activate(index);

        let pending = self.tasks[index].pending;
        self.tasks[index].pending = Pending::None;
        let mut blocked_again = false;

        match pending {
            Pending::Recv { handle } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv(tcb, handle) {
                    Ok(Some(received)) => {
                        let sender = self.endpoint_of(received.sender);
                        match self.install_handle(index, &received.message) {
                            Some(handle) => {
                                if received.reply.is_some() {
                                    self.tasks[index].reply = received.reply;
                                }
                                let context = &mut self.tasks[index].context;
                                context.set_result(received.message.label);
                                context.set_word(received.message.words[0]);
                                context.set_sender(sender);
                                context.set_handle(handle);
                            }
                            None => {
                                self.tasks[index].reply = None;
                                self.tasks[index].context.set_result(u64::MAX);
                            }
                        }
                    }
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::RecvData {
                handle,
                buffer,
                capacity,
                expected,
                handle_out,
                deadline,
            } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv_from(tcb, handle, expected) {
                    Ok(Some(received)) => {
                        let mut frame = self.tasks[index].context;
                        if handle_out {
                            match self.install_handle(index, &received.message) {
                                Some(number) => {
                                    let _ = self.deliver_data_handle(
                                        index, &mut frame, &received, buffer, capacity, number,
                                    );
                                }
                                None => frame.set_result(u64::MAX),
                            }
                        } else {
                            let _ =
                                self.deliver_data(index, &mut frame, &received, buffer, capacity);
                        }
                        self.tasks[index].context = frame;
                        self.remove_sleep(index);
                    }
                    Ok(None) => {
                        if self.expected_exited(expected) {
                            // 期望的发送者已退出：过滤后不会再有消息，返回错误
                            // 而不是把客户端重新挂回等待队列。
                            let _ = self.kernel.cancel_recv(tcb, handle);
                            self.tasks[index].context.set_result(u64::MAX);
                        } else if deadline.is_some_and(|at| self.contract_now() >= at) {
                            // 超时兜底：撤销本次登记，避免悬挂接收者。
                            let _ = self.kernel.cancel_recv(tcb, handle);
                            self.tasks[index].context.set_result(u64::MAX);
                        } else {
                            blocked_again = true;
                        }
                    }
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::Notify { handle, mask } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.wait(tcb, handle, mask) {
                    Ok(Some(bits)) => self.tasks[index].context.set_result(bits),
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::Call {
                endpoint, started, ..
            } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.take_reply(tcb) {
                    Some(message) => {
                        let now = self.platform.timestamp();
                        if now >= started {
                            let cycles = now - started;
                            self.call_count = self.call_count.saturating_add(1);
                            self.call_cycles = self.call_cycles.saturating_add(cycles);
                            self.call_max = self.call_max.max(cycles);
                        }
                        let Some(handle) = self.install_handle(index, &message) else {
                            self.tasks[index].context.set_result(u64::MAX);
                            self.remove_sleep(index);
                            return false;
                        };
                        let context = &mut self.tasks[index].context;
                        context.set_result(message.label);
                        context.set_word(message.words[0]);
                        context.set_sender(endpoint);
                        context.set_handle(handle);
                        self.remove_sleep(index);
                    }
                    None => blocked_again = true,
                }
            }
            Pending::ProviderReclaim {
                capability,
                index: page_index,
                object,
                deadline,
                provider,
                end,
                released,
                count,
            } => {
                let caller = self.tasks[index].tcb;
                match self.kernel.take_reply(caller) {
                    Some(message) => {
                        if message.label == hk_syscall::PROVIDER_RECLAIMED && message.words[3] == 0
                        {
                            let mut released = released;
                            for page in page_index..page_index + count {
                                released = released.saturating_add(
                                    self.finish_provider_reclaim(index, capability, object, page),
                                );
                            }
                            match self.continue_provider_reclaim(
                                index,
                                caller,
                                capability,
                                object,
                                provider,
                                page_index + count,
                                end,
                                released,
                            ) {
                                Some(total) => self.tasks[index].context.set_result(total),
                                None => blocked_again = true,
                            }
                        } else {
                            for page in page_index..page_index + count {
                                self.kernel.end_capability_reclaim(caller, capability, page);
                            }
                            self.tasks[index].context.set_result(u64::MAX);
                        }
                    }
                    None if deadline <= self.contract_now() => {
                        let _ = self.kernel.cancel_call(caller);
                        for page in page_index..page_index + count {
                            self.kernel.end_capability_reclaim(caller, capability, page);
                        }
                        self.tasks[index].context.set_result(u64::MAX);
                    }
                    None => blocked_again = true,
                }
            }
            Pending::ProviderFault {
                capability,
                region,
                region_user,
                index: page_index,
                page,
                rights,
                flags,
                access: _,
                private,
                deadline: _,
            } => {
                let caller = self.tasks[index].tcb;
                let Some(message) = self.kernel.take_reply(caller) else {
                    return self.fail_provider(cpu, index, pending);
                };
                if message.label != hk_syscall::PROVIDER_PROVIDE {
                    return self.fail_provider(cpu, index, pending);
                }
                let Some(frame) = message.handles[0].and_then(|handle| {
                    let frame = self.kernel.take_owned_frame(caller, handle).ok();
                    let _ = self.kernel.close_handle(caller, handle);
                    frame
                }) else {
                    return self.fail_provider(cpu, index, pending);
                };
                let range = frame.range();
                let page_size = hk_hal::PAGE_SIZE as u64;
                if range.len() != hk_hal::PAGE_SIZE as u64 {
                    return self.fail_provider(cpu, index, pending);
                }
                let Some(map) = self
                    .process(index)
                    .regions
                    .iter()
                    .flatten()
                    .find(|map| map.region == region && map.user == region_user)
                else {
                    return self.fail_provider(cpu, index, pending);
                };
                let owner_tcb = map.owner;
                let map_virt = map.virt;
                let map_len = map.len as u64;
                let map_offset = map.offset;
                let map_rights = map.rights;
                let map_flags = map.flags;
                if map_rights != rights
                    || map_flags != flags
                    || page < map_virt
                    || page.saturating_add(page_size) > map_virt.saturating_add(map_len)
                {
                    return self.fail_provider(cpu, index, pending);
                }
                let Ok((capability_info, _)) = self.kernel.capability(owner_tcb, capability) else {
                    return self.fail_provider(cpu, index, pending);
                };
                let Some(capability_offset) = page
                    .checked_sub(map_virt)
                    .and_then(|relative| map_offset.checked_add(relative))
                else {
                    return self.fail_provider(cpu, index, pending);
                };
                let Some(capability_end) = page_index
                    .checked_mul(hk_hal::PAGE_SIZE)
                    .and_then(|offset| offset.checked_add(hk_hal::PAGE_SIZE))
                else {
                    return self.fail_provider(cpu, index, pending);
                };
                if capability_offset != (page_index * hk_hal::PAGE_SIZE) as u64
                    || capability_end as u64 > capability_info.size()
                {
                    return self.fail_provider(cpu, index, pending);
                }

                let backing = hk_mem::hmp::FrameBacking::Owned(frame);
                let mut installed = range;
                let mut installed_new = false;
                let charge = if private {
                    let Ok(charge) = self.charge_frame_range(index, range) else {
                        return self.fail_provider(cpu, index, pending);
                    };
                    let Ok(id) = self.kernel.handle_id(owner_tcb, capability) else {
                        self.release_frame(charge);
                        return self.fail_provider(cpu, index, pending);
                    };
                    if !self.process_mut(index).insert_cow_page(
                        (id.get(), page_index),
                        crate::runtime::CowPage {
                            range,
                            backing: Some(Arc::new(crate::runtime::CowBacking { backing })),
                        },
                    ) {
                        self.release_frame(charge);
                        return self.fail_provider(cpu, index, pending);
                    }
                    self.kernel
                        .clear_capability_swapped(owner_tcb, capability, page_index);
                    charge
                } else {
                    if let Ok(Some(existing)) = self
                        .kernel
                        .capability_page(owner_tcb, capability, page_index)
                    {
                        // 另一个任务已经装入该页：复用常驻页，不覆盖、不释放。
                        installed = existing;
                    } else {
                        if self
                            .kernel
                            .set_capability_page(owner_tcb, capability, page_index, backing)
                            .unwrap_or(false)
                        {
                            installed_new = true;
                        } else if let Ok(Some(existing)) = self
                            .kernel
                            .capability_page(owner_tcb, capability, page_index)
                        {
                            // 并发的 Provider 回包抢先装入：新 Frame 已被丢弃，
                            // 当前任务改为映射既有常驻页。
                            installed = existing;
                        } else {
                            return self.fail_provider(cpu, index, pending);
                        }
                    }
                    self.kernel
                        .clear_capability_swapped(owner_tcb, capability, page_index);
                    let Ok(charge) = self.charge_frame_range(index, installed) else {
                        if installed_new {
                            let _ = self
                                .kernel
                                .take_capability_page(owner_tcb, capability, page_index);
                        }
                        return self.fail_provider(cpu, index, pending);
                    };
                    charge
                };
                let cache = self
                    .kernel
                    .capability(owner_tcb, capability)
                    .map(|(capability, _)| capability.cache())
                    .unwrap_or(hk_hal::CachePolicy::WriteBack);
                if !self.platform.map_region(
                    index,
                    &hk_mem::RegionMapping {
                        virt: page,
                        phys: installed.start().get(),
                        len: hk_hal::PAGE_SIZE,
                        rights: map_rights,
                        flags: map_flags,
                        policy: cache,
                    },
                ) {
                    if private {
                        if let Ok(id) = self.kernel.handle_id(owner_tcb, capability) {
                            self.process_mut(index)
                                .cow_pages
                                .remove(&(id.get(), page_index));
                        }
                    } else if installed_new {
                        let _ = self
                            .kernel
                            .take_capability_page(owner_tcb, capability, page_index);
                    }
                    self.release_frame(charge);
                    return self.fail_provider(cpu, index, pending);
                }
                self.record_page_charge_for_capability(index, capability, page_index, charge);
                // Provider 提供页内容；内核只安装 PTE，不能再次清零。
            }
            Pending::None | Pending::Send { .. } | Pending::Sleep { .. } => {}
        }

        if blocked_again {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }

        blocked_again
    }

    /// Provider 回复缺失或安装失败：按缺页故障终止，而不是消费回复后永久阻塞。
    fn fail_provider(&mut self, cpu: usize, index: usize, pending: Pending) -> bool {
        if !self.fault(cpu, PROVIDER_FAULT_EXIT) {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }
        true
    }
}
