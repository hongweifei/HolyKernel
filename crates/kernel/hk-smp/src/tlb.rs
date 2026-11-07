//! 跨 CPU TLB shootdown：代际邮箱与确认。

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// 表示刷新整个 TLB。
const FULL_FLUSH: u64 = u64::MAX;

/// 平台提供的 TLB/IPI 硬件操作。
pub trait TlbHardware {
    /// 判断槽位是否在线。
    fn online(&self, cpu: usize) -> bool;
    /// 向槽位发送 IPI。
    fn send_ipi(&mut self, cpu: usize);
    /// 刷新当前 CPU 的 TLB。
    fn flush(&mut self, address: Option<u64>);
    /// 返回单调时间戳。
    fn now(&mut self) -> u64;
}

/// 每 CPU 的 TLB shootdown 状态。
pub struct TlbShootdown<const N: usize> {
    address: [AtomicU64; N],
    target: [AtomicU64; N],
    acknowledged: [AtomicU64; N],
    generation: AtomicU64,
    completed: AtomicUsize,
}

impl<const N: usize> TlbShootdown<N> {
    /// 构造空状态。
    pub const fn new() -> Self {
        Self {
            address: [const { AtomicU64::new(0) }; N],
            target: [const { AtomicU64::new(0) }; N],
            acknowledged: [const { AtomicU64::new(0) }; N],
            generation: AtomicU64::new(1),
            completed: AtomicUsize::new(0),
        }
    }

    /// 请求一次 shootdown，等待其他在线 CPU 确认。
    ///
    /// `timeout` 与 [`TlbHardware::now`] 使用同一时间单位。
    pub fn shootdown<H: TlbHardware>(
        &self,
        current: usize,
        address: Option<u64>,
        hardware: &mut H,
        timeout: u64,
    ) -> bool {
        if current >= N {
            return false;
        }

        hardware.flush(address);
        let requested = address.unwrap_or(FULL_FLUSH);
        let mut targets = [0_u64; N];

        for (cpu, slot) in targets.iter_mut().enumerate() {
            if cpu == current || !hardware.online(cpu) {
                continue;
            }

            // 已有未确认请求时提升为整表刷新，避免后一个请求覆盖前一个地址。
            // 每次重试都取更大的代际，防止并发请求把目标代际改小。
            let mut previous = self.target[cpu].load(Ordering::Acquire);
            let generation = loop {
                let flush = if previous > self.acknowledged[cpu].load(Ordering::Acquire) {
                    FULL_FLUSH
                } else {
                    requested
                };
                let candidate = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
                self.address[cpu].store(flush, Ordering::Release);
                match self.target[cpu].compare_exchange(
                    previous,
                    candidate,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break candidate,
                    Err(actual) => previous = actual,
                }
            };

            *slot = generation;
            hardware.send_ipi(cpu);
        }

        let begin = hardware.now();
        while !targets.iter().enumerate().all(|(cpu, generation)| {
            *generation == 0 || self.acknowledged[cpu].load(Ordering::Acquire) >= *generation
        }) {
            // 另一个 CPU 也可能正在等待本 CPU 确认，轮询时不依赖中断。
            self.acknowledge(current, |flush| hardware.flush(flush));
            if hardware.now().wrapping_sub(begin) >= timeout {
                return false;
            }
            core::hint::spin_loop();
        }

        self.completed.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// 处理 `cpu` 当前的 shootdown 请求并写入确认。
    pub fn acknowledge<F>(&self, cpu: usize, mut flush: F)
    where
        F: FnMut(Option<u64>),
    {
        if cpu >= N {
            return;
        }
        let target = self.target[cpu].load(Ordering::Acquire);
        if target <= self.acknowledged[cpu].load(Ordering::Acquire) {
            return;
        }

        let address = self.address[cpu].load(Ordering::Acquire);
        flush((address != FULL_FLUSH).then_some(address));
        self.acknowledged[cpu].store(target, Ordering::Release);
    }

    /// 返回已完成的 shootdown 次数。
    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Acquire)
    }
}

impl<const N: usize> Default for TlbShootdown<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    struct Mock<'a> {
        tlb: &'a TlbShootdown<4>,
        flushed: Cell<usize>,
        now: Cell<u64>,
        ack: bool,
    }

    impl TlbHardware for Mock<'_> {
        fn online(&self, cpu: usize) -> bool {
            cpu != 0
        }

        fn send_ipi(&mut self, cpu: usize) {
            if self.ack {
                self.tlb.acknowledge(cpu, |_| {});
            }
        }

        fn flush(&mut self, _address: Option<u64>) {
            self.flushed.set(self.flushed.get() + 1);
        }

        fn now(&mut self) -> u64 {
            let now = self.now.get() + 1;
            self.now.set(now);
            now
        }
    }

    #[test]
    fn waits_for_acknowledgements() {
        let tlb = TlbShootdown::<4>::new();
        let mut hardware = Mock {
            tlb: &tlb,
            flushed: Cell::new(0),
            now: Cell::new(0),
            ack: true,
        };
        assert!(tlb.shootdown(0, Some(0x1000), &mut hardware, 100));
        assert_eq!(hardware.flushed.get(), 1);
        assert_eq!(tlb.completed(), 1);
    }

    #[test]
    fn times_out_without_acknowledgement() {
        let tlb = TlbShootdown::<4>::new();
        let mut hardware = Mock {
            tlb: &tlb,
            flushed: Cell::new(0),
            now: Cell::new(0),
            ack: false,
        };
        assert!(!tlb.shootdown(0, None, &mut hardware, 3));
        assert_eq!(tlb.completed(), 0);
    }
}
