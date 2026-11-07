use super::*;

impl Kernel {
    /// 创建线程；线程获得独立的句柄表并立即可运行。
    pub fn spawn_thread(&self, handles: HandleTable, priority: Priority) -> TcbId {
        let id = self.spawn_parked(handles, priority);
        let _ = self.activate(id);
        id
    }

    /// 创建线程但不进入调度队列，等待 [`Self::activate`]。
    pub fn spawn_parked(&self, handles: HandleTable, priority: Priority) -> TcbId {
        let id = TcbId::new(self.next_tcb.fetch_add(1, Ordering::Relaxed));
        let mut tcb = Tcb::new(id, priority, handles);
        tcb.set_state(ThreadState::Inactive);
        self.threads.lock().insert(id, tcb);
        self.record(id.get(), Category::Sched, TRACE_THREAD_SPAWN, 0);
        id
    }

    /// 把停泊线程置为可运行并加入调度队列。
    pub fn activate(&self, thread: TcbId) -> Result<(), KernelError> {
        let priority = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            if tcb.state() != ThreadState::Inactive {
                return Ok(());
            }
            tcb.set_state(ThreadState::Runnable);
            tcb.priority()
        };
        self.scheduler.lock().enqueue(thread, priority);
        Ok(())
    }

    /// 把线程停泊并移出调度队列。
    pub fn suspend(&self, thread: TcbId) -> Result<(), KernelError> {
        {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.set_state(ThreadState::Inactive);
        }
        let mut scheduler = self.scheduler.lock();
        scheduler.remove(thread);
        scheduler.clear_current_if(thread);
        Ok(())
    }

    /// 判断是否有排队等待调度的线程。
    pub fn has_runnable(&self) -> bool {
        self.scheduler.lock().runnable() > 0
    }

    /// 取 `cpu` 的下一个可运行线程；该 CPU 的当前线程按同优先级轮转回就绪队列。
    pub fn schedule(&self, cpu: usize) -> Option<TcbId> {
        let current = self.scheduler.lock().take_current(cpu);
        if let Some(current) = current {
            let requeue = {
                let mut threads = self.threads.lock();
                match threads.get_mut(&current) {
                    Some(tcb) if tcb.state() == ThreadState::Running => {
                        tcb.set_state(ThreadState::Runnable);
                        Some(tcb.priority())
                    }
                    _ => None,
                }
            };
            if let Some(priority) = requeue {
                self.scheduler.lock().enqueue(current, priority);
            }
        }

        let next = self.scheduler.lock().pick(cpu)?;
        {
            let mut threads = self.threads.lock();
            if let Some(tcb) = threads.get_mut(&next) {
                tcb.set_state(ThreadState::Running);
            }
        }
        self.record(next.get(), Category::Sched, TRACE_DISPATCH, 0);
        Some(next)
    }

    /// 终止线程；句柄表随 TCB 一起释放。
    pub fn exit(&self, thread: TcbId) -> Result<(), KernelError> {
        let tcb = self
            .threads
            .lock()
            .remove(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        let live = tcb.handles().live();
        drop(tcb);
        {
            let mut scheduler = self.scheduler.lock();
            scheduler.remove(thread);
            scheduler.clear_current_if(thread);
        }
        self.record(thread.get(), Category::Sched, TRACE_EXIT, 0);
        self.record(
            thread.get(),
            Category::Handle,
            TRACE_HANDLE_RELEASE,
            live as u64,
        );
        Ok(())
    }

    /// 唤醒被阻塞的线程。
    pub fn wake_thread(&self, thread: TcbId) {
        self.wake(thread);
    }

    /// 挂起线程，等待用户态运行库唤醒。
    pub fn park(&self, thread: TcbId) -> Result<(), KernelError> {
        self.block(thread, BlockReason::Park)
    }

    pub(crate) fn block(&self, thread: TcbId, reason: BlockReason) -> Result<(), KernelError> {
        {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.set_state(ThreadState::Blocked);
            tcb.set_blocked(Some(reason));
        }
        let mut scheduler = self.scheduler.lock();
        scheduler.remove(thread);
        scheduler.clear_current_if(thread);
        drop(scheduler);
        self.record(thread.get(), Category::Sched, TRACE_BLOCK, 0);
        Ok(())
    }

    pub(crate) fn wake(&self, thread: TcbId) {
        let priority = {
            let mut threads = self.threads.lock();
            let Some(tcb) = threads.get_mut(&thread) else {
                return;
            };
            if tcb.state() != ThreadState::Blocked {
                return;
            }
            tcb.set_state(ThreadState::Runnable);
            tcb.set_blocked(None);
            tcb.priority()
        };
        self.scheduler.lock().enqueue(thread, priority);
        self.record(thread.get(), Category::Sched, TRACE_WAKE, 0);
    }
}
