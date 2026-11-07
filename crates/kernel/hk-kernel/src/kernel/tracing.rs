use super::*;

impl Kernel {
    pub(crate) fn record(&self, thread: u64, category: Category, code: u32, argument: u64) {
        let moment = self.moment.fetch_add(1, Ordering::Relaxed) + 1;
        self.trace
            .lock()
            .record(moment, 0, thread, category, code, argument);
    }

    /// 返回最近的 trace 事件。
    pub fn trace_events(&self, limit: usize) -> Vec<crate::trace::Event> {
        self.trace.lock().latest(limit).copied().collect()
    }

    /// 返回 trace 事件数量。
    pub fn trace_len(&self) -> usize {
        self.trace.lock().len()
    }
}
