//! AArch64 原语：`aarch64-cpu` crate 的薄封装。

use aarch64_cpu::asm::barrier::{SY, dsb, isb};

/// 中断是否开启（DAIF.I 未置位）。
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: `mrs` 只读取当前 EL 的 DAIF。
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
    }
    daif & (1 << 7) == 0
}

/// 开启 IRQ。
pub fn enable_interrupts() {
    // SAFETY: `daifclr #2` 只清除 IRQ 屏蔽位。
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
}

/// 关闭 IRQ。
pub fn disable_interrupts() {
    // SAFETY: `daifset #2` 只设置 IRQ 屏蔽位。
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }
}

/// 等待下一次中断。
pub fn halt() {
    aarch64_cpu::asm::wfi();
}

/// 开启中断并等待；唤醒后中断保持开启。
pub fn enable_and_halt() {
    enable_interrupts();
    halt();
}

/// 全内存屏障。
pub fn fence() {
    dsb(SY);
    isb(SY);
}
