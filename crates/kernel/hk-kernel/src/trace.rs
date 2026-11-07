//! trace 环形缓冲。
//!
//! 每 CPU 一个 [`TraceRing`]，事件带序号、时间戳、CPU、线程、类别与诊断码。
//! 环形缓冲固定容量，覆盖旧事件，适合 panic dump 与确定性回放。

/// 事件类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// 启动流程。
    Boot,
    /// IPC 发送、接收、调用与回复。
    Ipc,
    /// 调度、阻塞与唤醒。
    Sched,
    /// 内存对象与映射。
    Mem,
    /// 句柄授予、复制与关闭。
    Handle,
    /// VFS 请求。
    Vfs,
    /// 网络。
    Net,
    /// POSIX 语义。
    Posix,
    /// 服务器生命周期。
    Server,
    /// 显示与界面。
    Ui,
}

impl Category {
    /// 返回类别名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Ipc => "ipc",
            Self::Sched => "sched",
            Self::Mem => "mem",
            Self::Handle => "handle",
            Self::Vfs => "vfs",
            Self::Net => "net",
            Self::Posix => "posix",
            Self::Server => "server",
            Self::Ui => "ui",
        }
    }
}

/// trace 事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// 全局递增序号。
    pub sequence: u64,
    /// 发生时的时间戳。
    pub moment: u64,
    /// CPU 序号。
    pub cpu: u32,
    /// 线程编号。
    pub thread: u64,
    /// 事件类别。
    pub category: Category,
    /// 诊断码，取值由类别定义。
    pub code: u32,
    /// 附加参数，例如对象标识或消息标签。
    pub argument: u64,
}

/// 固定容量的 trace 环形缓冲。
pub struct TraceRing<const N: usize> {
    events: [Option<Event>; N],
    next: usize,
    sequence: u64,
    count: usize,
}

impl<const N: usize> TraceRing<N> {
    /// 构造空环形缓冲。
    pub const fn new() -> Self {
        Self {
            events: [None; N],
            next: 0,
            sequence: 1,
            count: 0,
        }
    }

    /// 记录事件。
    pub fn record(
        &mut self,
        moment: u64,
        cpu: u32,
        thread: u64,
        category: Category,
        code: u32,
        argument: u64,
    ) {
        let event = Event {
            sequence: self.sequence,
            moment,
            cpu,
            thread,
            category,
            code,
            argument,
        };
        self.sequence += 1;
        self.events[self.next] = Some(event);
        self.next = (self.next + 1) % N;
        if self.count < N {
            self.count += 1;
        }
    }

    /// 返回已记录事件数，上限为容量。
    pub fn len(&self) -> usize {
        self.count
    }

    /// 判断缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 判断缓冲是否已写满。
    pub fn is_full(&self) -> bool {
        self.count == N
    }

    /// 按时间顺序遍历事件，最旧的在前。
    pub fn iter(&self) -> impl Iterator<Item = &Event> {
        let start = self.next;
        (0..N).filter_map(move |offset| self.events[(start + offset) % N].as_ref())
    }

    /// 返回最近 `count` 个事件。
    pub fn latest(&self, count: usize) -> impl Iterator<Item = &Event> {
        let total = self.len();
        let skip = total.saturating_sub(count);
        self.iter().skip(skip)
    }
}

impl<const N: usize> Default for TraceRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn records_events_in_order() {
        let mut trace = TraceRing::<8>::new();
        trace.record(10, 0, 1, Category::Ipc, 1, 100);
        trace.record(20, 0, 2, Category::Sched, 2, 200);

        let values: Vec<_> = trace.iter().copied().collect();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].sequence, 1);
        assert_eq!(values[0].category, Category::Ipc);
        assert_eq!(values[1].argument, 200);
    }

    #[test]
    fn wraps_around_and_keeps_newest() {
        let mut trace = TraceRing::<4>::new();
        for index in 0..6 {
            trace.record(index, 0, 1, Category::Sched, index as u32, 0);
        }

        assert_eq!(trace.len(), 4);
        assert!(trace.is_full());

        let moments: Vec<_> = trace.iter().map(|event| event.moment).collect();
        assert_eq!(moments, [2, 3, 4, 5]);
    }

    #[test]
    fn latest_returns_tail() {
        let mut trace = TraceRing::<8>::new();
        for index in 0..5 {
            trace.record(index, 0, 1, Category::Ipc, 0, index);
        }

        let arguments: Vec<_> = trace.latest(2).map(|event| event.argument).collect();
        assert_eq!(arguments, [3, 4]);
    }
}
