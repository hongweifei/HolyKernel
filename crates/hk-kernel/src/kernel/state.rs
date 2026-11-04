use super::*;

impl Kernel {
    /// 返回线程状态。
    pub fn state_of(&self, thread: TcbId) -> Option<ThreadState> {
        self.threads.lock().get(&thread).map(Tcb::state)
    }

    /// 返回线程数量。
    pub fn thread_count(&self) -> usize {
        self.threads.lock().len()
    }

    /// 返回存活对象数量。
    pub fn live_objects(&self) -> usize {
        self.objects
            .lock()
            .values()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    /// 返回就绪线程数量。
    pub fn runnable(&self) -> usize {
        self.scheduler.lock().runnable()
    }

    /// 返回句柄表中的在用句柄数。
    pub fn handles_live(&self, thread: TcbId) -> Option<usize> {
        self.threads
            .lock()
            .get(&thread)
            .map(|tcb| tcb.handles().live())
    }

    /// 生成内核状态报告。
    ///
    /// panic dump 与测试断言使用同一份报告，保证故障现场可复现。
    pub fn dump(&self) -> String {
        let mut text = String::new();
        let runnable = self.runnable();
        let _ = writeln!(
            text,
            "kernel: threads={} objects={} runnable={}",
            self.thread_count(),
            self.live_objects(),
            runnable
        );

        for tcb in self.threads.lock().values() {
            let _ = writeln!(
                text,
                "  tcb={} state={:?} priority={} handles={} blocked={:?} pending={}",
                tcb.id().get(),
                tcb.state(),
                tcb.priority().level(),
                tcb.handles().live(),
                tcb.blocked(),
                tcb.pending_deliveries()
            );
        }

        let trace = self.trace.lock();
        let _ = writeln!(text, "trace: events={}", trace.len());
        for event in trace.latest(8) {
            let _ = writeln!(
                text,
                "  seq={} cat={} thread={} code={} arg={:#x}",
                event.sequence,
                event.category.name(),
                event.thread,
                event.code,
                event.argument
            );
        }
        text
    }
}
