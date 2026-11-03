//! 虚拟中断控制器。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use hk_hal::{CpuId, InterruptController, Ipi, Irq, IrqError, NotificationId};

/// Hosted 平台的中断绑定表。
#[derive(Debug)]
pub struct HostedInterruptController {
    state: Mutex<IrqState>,
}

#[derive(Debug, Default)]
struct IrqState {
    bindings: BTreeMap<u32, NotificationId>,
    masked: BTreeSet<u32>,
}

impl HostedInterruptController {
    /// 构造中断控制器。
    pub fn new() -> Self {
        Self {
            state: Mutex::new(IrqState::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, IrqState> {
        self.state.lock().expect("中断控制器锁中毒")
    }

    /// 返回中断号是否被屏蔽。
    pub fn is_masked(&self, irq: Irq) -> bool {
        self.lock().masked.contains(&irq.get())
    }

    /// 返回中断号绑定的 Notification。
    pub fn binding(&self, irq: Irq) -> Option<NotificationId> {
        self.lock().bindings.get(&irq.get()).copied()
    }
}

impl Default for HostedInterruptController {
    fn default() -> Self {
        Self::new()
    }
}

impl InterruptController for HostedInterruptController {
    fn bind(&self, irq: Irq, target: NotificationId) -> Result<(), IrqError> {
        let mut state = self.lock();
        if state.bindings.contains_key(&irq.get()) {
            return Err(IrqError::AlreadyBound);
        }
        state.bindings.insert(irq.get(), target);
        Ok(())
    }

    fn unbind(&self, irq: Irq) -> Result<(), IrqError> {
        let mut state = self.lock();
        match state.bindings.remove(&irq.get()) {
            Some(_) => Ok(()),
            None => Err(IrqError::NotBound),
        }
    }

    fn mask(&self, irq: Irq) {
        self.lock().masked.insert(irq.get());
    }

    fn unmask(&self, irq: Irq) {
        self.lock().masked.remove(&irq.get());
    }

    fn eoi(&self, _irq: Irq) {
        // Hosted 平台没有真实中断控制器，结束信号无需硬件动作。
    }

    fn send_ipi(&self, _cpu: CpuId, _ipi: Ipi) {
        // 单进程模拟器在单线程事件循环中调度 CPU，跨核 IPI 由事件循环处理。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_and_unbinds_interrupts() {
        let controller = HostedInterruptController::new();
        let irq = Irq::new(4);

        assert_eq!(controller.bind(irq, NotificationId::new(7)), Ok(()));
        assert_eq!(controller.binding(irq), Some(NotificationId::new(7)));
        assert_eq!(
            controller.bind(irq, NotificationId::new(8)),
            Err(IrqError::AlreadyBound)
        );

        assert_eq!(controller.unbind(irq), Ok(()));
        assert_eq!(controller.unbind(irq), Err(IrqError::NotBound));
    }

    #[test]
    fn masks_interrupts() {
        let controller = HostedInterruptController::new();
        let irq = Irq::new(9);

        assert!(!controller.is_masked(irq));
        controller.mask(irq);
        assert!(controller.is_masked(irq));
        controller.unmask(irq);
        assert!(!controller.is_masked(irq));
    }
}
