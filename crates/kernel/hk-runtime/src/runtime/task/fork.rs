//! fork：写时复制、句柄视图与子任务初始化。

use alloc::format;
use alloc::vec::Vec;

use hk_cadence::DEFAULT_SPEC;
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::{HandleTable, Rights};
use hk_kernel::TcbId;

use crate::runtime::{Process, Switch, TaskInit, UserPlatform, UserRuntime, UserTask};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 复制当前任务：页表写时复制、句柄与通知映射；子任务返回 0。
    pub(crate) fn fork_task(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        if !self.populate_lazy_regions(index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let child_index = self.tasks.len();
        // PTE 在 fork 时统一降为只读；父任务 Region 必须同步标记 COW，
        // 否则写缺页会被当成保护错误，无法按私有副本解析。
        for region in self.process_mut(index).regions.iter_mut().flatten() {
            if region.rights.contains(hk_handle::Rights::WRITE)
                && !region.flags.contains(hk_mem::hmp::RegionFlags::DONT_FORK)
            {
                region.flags.insert(hk_mem::hmp::RegionFlags::COW);
            }
        }
        let cow_ranges: Vec<(u64, u64)> = self
            .process(index)
            .regions
            .iter()
            .flatten()
            .filter(|region| {
                region.rights.contains(hk_handle::Rights::WRITE)
                    && !region.flags.contains(hk_mem::hmp::RegionFlags::DONT_FORK)
            })
            .map(|region| (region.virt, region.len as u64))
            .collect();
        let capacity = self.task_capacity(index);
        let Some(forked) = self
            .platform
            .fork(index, child_index, &cow_ranges, capacity.as_ref())
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let child = match kernel.task_of(tcb) {
            Some(parent) => kernel
                .spawn_task_child_parked(HandleTable::with_default_quota(), parent)
                .expect("子任务创建"),
            None => kernel
                .spawn_task_parked(HandleTable::with_default_quota(), DEFAULT_SPEC)
                .expect("子任务创建"),
        };

        let Ok(own_endpoint) = kernel.create_endpoint(child, rights) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(own_endpoint_id) = kernel.handle_id(child, own_endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        self.endpoints.push(Some(own_endpoint_id));

        let Ok(own_notification) = kernel.create_notification(child, rights) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(own_notification_id) = kernel.handle_id(child, own_notification) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        self.notifications.push(Some(own_notification_id));

        let (handles, notifications) = self.endpoint_views(
            child,
            child_index,
            Some(own_endpoint),
            Some(own_notification),
        );
        self.append_endpoint(own_endpoint_id, own_notification_id);

        let mut context = *frame;
        context.set_result(0);
        let name = format!("{}-fork", self.tasks[index].name);
        let arena_base = self.process(index).arena_base;
        let arena_size = self.process(index).arena_size;
        let arena_used = self.process(index).arena_used();
        let ledger_limit = self.process(index).ledger.limit();
        let parent_capacity = self.process(index).allocation_capacity;
        let Ok(capacity_object) = self.kernel.handle_id(tcb, parent_capacity) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(capacity_rights) =
            self.kernel
                .handle_rights(tcb, parent_capacity, hk_handle::ObjectKind::Capacity)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let child_capacity = match self.kernel.grant(child, capacity_object, capacity_rights) {
            Ok(handle) => handle,
            Err(_) => {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        let mut process = Process::new(arena_base, arena_size, ledger_limit, child_capacity);
        if let Some(task) = self.kernel.task_of(child) {
            let _ = self.kernel.set_task_ledger(task, process.ledger.clone());
        }
        process.set_arena_used(arena_used);
        // 子进程继承父进程的私有页视图，但不继承记账与所有权：物理页仍由
        // 父进程持有，子进程写入时再复制自己的页。
        process.cow_pages = self
            .process(index)
            .cow_pages
            .iter()
            .map(|(key, page)| {
                (
                    *key,
                    crate::runtime::CowPage {
                        range: page.range,
                        backing: None,
                    },
                )
            })
            .collect();
        for range in &forked.pages {
            let Ok(charge) =
                hk_mem::hmp::frame_ledger::charge_range(self.frame_scope, &process.ledger, *range)
            else {
                for charge in process.cow_charges.drain(..) {
                    hk_mem::hmp::frame_ledger::release(charge);
                }
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            process.cow_charges.push(charge);
        }
        let mut task = UserTask::new(TaskInit {
            tcb: child,
            process: child_index,
            name,
            parent: Some(tcb.get()),
            handles,
            notifications,
            context,
        });
        task.leader = child_index;
        task.capacities.push(Some(child_capacity));
        self.tasks.push(task);
        self.processes.push(Some(process));
        self.fork_regions(index, child_index);
        self.prune_all_cow_pages(child_index);
        let _ = self.kernel.activate(child);

        frame.set_result(child_index as u64 + 1);
        Switch::Continue
    }
}
