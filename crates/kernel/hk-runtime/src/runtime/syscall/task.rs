use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn allocate(&mut self, index: usize, size: usize, align: usize) -> Option<u64> {
        // 线程共享进程 Arena，避免每个线程各自占用一段未映射的地址空间。
        let slot = self.leader_of(index);
        let (start, len, before) = {
            let process = self.process_mut(slot);
            let before = process.arena_used();
            let (start, len) = process.allocate_arena(size, align)?;
            (start, len, before)
        };
        // 每次分配建立独立的惰性匿名 Region；首次访问装入物理页时才记账。
        let spec = UserRegionSpec {
            virt: start,
            len,
            writable: true,
            executable: false,
        };
        if self.register_region(slot, spec, None).is_err() {
            self.process_mut(slot).set_arena_used(before);
            return None;
        }
        Some(start)
    }

    pub(crate) fn notification_handle(&self, index: usize, number: usize) -> Option<Handle> {
        self.tasks[index]
            .notifications
            .get(number.wrapping_sub(1))
            .copied()
            .flatten()
    }

    /// 登记端点接收内核投递的原始异常现场。
    pub(crate) fn fault_subscribe(&mut self, index: usize, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self
            .fault_subscribers
            .iter()
            .any(|(slot, existing)| *slot == index && *existing == handle)
        {
            self.fault_subscribers.push((index, handle));
        }
        frame.set_result(0);
        Switch::Continue
    }

    /// 把随消息收到的句柄加入任务额外句柄表，返回编号；没有句柄时为 0。
    pub(crate) fn close_handle(&mut self, index: usize, tcb: TcbId, endpoint: usize) -> u64 {
        let Some(handle) = self.handle_at(index, endpoint) else {
            return u64::MAX;
        };
        if (REGION_HANDLE_BASE..CAPACITY_HANDLE_BASE).contains(&endpoint) {
            let Some(slot) = endpoint.checked_sub(REGION_HANDLE_BASE + 1) else {
                return u64::MAX;
            };
            if self
                .process(index)
                .regions
                .get(slot)
                .and_then(|region| region.as_ref())
                .is_none()
            {
                return u64::MAX;
            }
            if !self.teardown_region(index, slot, true, false) {
                return u64::MAX;
            }
            return 0;
        }
        if (CAPABILITY_HANDLE_BASE..REGION_HANDLE_BASE).contains(&endpoint) {
            // 关闭 capability 前先解除所有基于它的 Region，避免 PTE 继续指向
            // 已经随 capability 对象释放的物理页。
            let slots: Vec<usize> = self
                .process(index)
                .regions
                .iter()
                .enumerate()
                .filter_map(|(slot, region)| {
                    region
                        .as_ref()
                        .filter(|region| region.capability == handle)
                        .map(|_| slot)
                })
                .collect();
            for slot in slots {
                let _ = self.teardown_region(index, slot, false, false);
            }
        }

        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                self.clear_handle(index, endpoint);
                0
            }
            Err(_) => u64::MAX,
        }
    }
}
