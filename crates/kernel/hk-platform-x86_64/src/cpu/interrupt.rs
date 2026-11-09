//! 中断分派。

use super::apic::lapic_write;
use super::smp::{handle_tlb_ipi, note_ipi, take_resched};
use super::syscall::{handle_syscall, preempt};
use super::timer::note_timer_interrupt;
use super::user::{dispatch_fault, handle_mce, handle_nmi, leave_user, report_exception};
use super::*;

/// 中断分派：定时器计数、键盘输入、系统调用分派，其余向量按异常处理。
#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    if frame.vector == TIMER_VECTOR {
        note_timer_interrupt();
        let switch = frame.cs == u64::from(USER_CODE_SELECTOR) && preempt(frame);
        lapic_write(LAPIC_EOI, 0);
        if switch {
            leave_user();
        }
        return;
    }
    if frame.vector == crate::board::KEYBOARD_VECTOR {
        crate::keyboard::handle_interrupt();
        lapic_write(LAPIC_EOI, 0);
        return;
    }
    if frame.vector == SYSCALL_VECTOR {
        handle_syscall(frame);
        return;
    }
    if frame.vector == 2 {
        handle_nmi(frame);
        return;
    }
    if frame.vector == 18 {
        handle_mce(frame);
        return;
    }
    if frame.vector == SCI_VECTOR {
        crate::bootstrap::handle_sci();
        lapic_write(LAPIC_EOI, 0);
        return;
    }
    if frame.vector == HPET_VECTOR {
        crate::hpet::note_periodic();
        lapic_write(LAPIC_EOI, 0);
        return;
    }
    if (u64::from(crate::irq::IRQ_VECTOR_BASE)
        ..u64::from(crate::irq::IRQ_VECTOR_BASE + crate::irq::IRQ_LINES))
        .contains(&frame.vector)
    {
        crate::irq::note((frame.vector - u64::from(crate::irq::IRQ_VECTOR_BASE)) as u8);
        lapic_write(LAPIC_EOI, 0);
        return;
    }
    if frame.vector == u64::from(IPI_VECTOR) {
        note_ipi();
        handle_tlb_ipi();
        let resched = take_resched(cpu_index());
        let switch = resched && frame.cs == u64::from(USER_CODE_SELECTOR) && preempt(frame);
        lapic_write(LAPIC_EOI, 0);
        if switch {
            leave_user();
        }
        return;
    }

    if frame.cs == u64::from(USER_CODE_SELECTOR)
        && let Some(action) = dispatch_fault(frame)
    {
        match action {
            FaultAction::Resume => return,
            FaultAction::Exit | FaultAction::Block => leave_user(),
        }
    }

    report_exception(frame);
}
