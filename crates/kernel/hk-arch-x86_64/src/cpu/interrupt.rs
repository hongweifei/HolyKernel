//! 中断分派。

use core::fmt::Write;

use hk_hal::Console;

use crate::serial::{COM1, Serial};

use super::apic::{cpu_index, lapic_write};
use super::smp::{handle_tlb_ipi, note_ipi, take_resched};
use super::syscall::{handle_syscall, preempt};
use super::timer::note_timer_interrupt;
use super::user::{
    FaultAction, dispatch_fault, handle_mce, handle_nmi, leave_user, report_exception,
};
use super::{
    HPET_VECTOR, IPI_VECTOR, InterruptFrame, LAPIC_EOI, SCI_VECTOR, SYSCALL_VECTOR, TIMER_VECTOR,
    USER_CODE_SELECTOR,
};

/// 中断分派：定时器计数、键盘输入、系统调用分派，其余向量按异常处理。
#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    // 用户态可能把 DF 留在置位状态；入口桩已经 `cld`，这里只做一次性确认，
    // 便于定位是哪个用户任务泄漏了方向标志。
    if frame.rflags & (1 << 10) != 0 {
        static DF_SEEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        let seen = DF_SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if seen < 8 {
            struct DfReport(Serial);
            impl core::fmt::Write for DfReport {
                fn write_str(&mut self, text: &str) -> core::fmt::Result {
                    self.0.write_str(text);
                    Ok(())
                }
            }
            let mut report = DfReport(Serial::new(COM1));
            let _ = writeln!(
                report,
                "\n[df] 用户态方向标志置位：cpu={} vector={} rip={:#x} cs={:#x}",
                cpu_index(),
                frame.vector,
                frame.rip,
                frame.cs
            );
        }
    }
    // #DB 必须先于调试寄存器同步处理：同步会清 DR6，掩盖命中原因。
    if frame.vector == 1 {
        if super::watchpoint::report(frame) {
            return;
        }
    }
    super::watchpoint::sync();
    reject_heap_stack(frame);
    if frame.vector == TIMER_VECTOR {
        let oneshot = note_timer_interrupt();
        let mut switch = false;
        // 中断发生在内核态时保留抢占标志，由系统调用返回路径消费。
        if frame.cs == u64::from(USER_CODE_SELECTOR) {
            let _ = take_resched(cpu_index());
            switch = preempt(frame);
        } else if oneshot {
            // 一次性定时器在内核态到期：LVT 已被改成 one-shot，若就此丢弃，
            // 该 CPU 会失去唯一的调度定时器。保留一次重调度请求，并重新臂一个
            // 短 watchdog，让下一次到期回到用户态时完成抢占。
            super::smp::note_resched(cpu_index());
            super::arm_oneshot(1_000);
        }
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
        super::watchpoint::note_syscall(cpu_index(), frame.rax);
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
        let switch = frame.cs == u64::from(USER_CODE_SELECTOR)
            && take_resched(cpu_index())
            && preempt(frame);
        lapic_write(LAPIC_EOI, 0);
        if switch {
            leave_user();
        }
        return;
    }

    // 未注册的硬件向量（含 LAPIC 伪中断 0xFF）不能按异常处理；否则
    // `report_exception` 会以无效向量索引异常名表并级联成三重故障。
    if frame.vector >= 32 {
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

/// 中断现场落在内核堆区间内时立即终止。
///
/// 中断栈与内核堆共用同一段物理内存时，ISR 的每一次 `call` 都会把返回地址写进
/// 堆对象，之后只会以“坏释放”或字段变成代码地址的形式晚到很久才暴露。这里在
/// 现场地址一进堆就报出来，并带上向量、CS 与地址。
///
/// 逐页物理核对只在 debug 构建开启：release 下这些绑定会被裁掉，允许其未使用。
/// 现场页本身的物理核对在两种构建都保留：每次中断只做一次地址翻译，代价可接受，
/// 而它正好覆盖“中断帧所在栈页与堆页同一物理页”的别名。
#[cfg_attr(not(debug_assertions), allow(unused_variables, unused_mut))]
fn reject_heap_stack(frame: &InterruptFrame) {
    struct Report(Serial);

    impl core::fmt::Write for Report {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            self.0.write_str(text);
            Ok(())
        }
    }

    let Some((heap_phys, heap_virt, heap_len)) = crate::paging::kernel_heap() else {
        return;
    };
    let in_heap_phys =
        |phys: u64| phys >= heap_phys && phys < heap_phys.saturating_add(heap_len);
    let address = frame as *const InterruptFrame as u64;
    let in_heap = |start: u64| address >= start && address < start.saturating_add(heap_len);
    let mut overlaps = in_heap(heap_virt);
    // 虚拟区间不同但物理帧相同的别名同样会写坏堆，这里翻译当前页表核对物理归属。
    if !overlaps {
        overlaps = crate::paging::translate_current(hk_hal::VirtAddr::new(address & !0xFFF))
            .is_some_and(|phys| in_heap_phys(phys.get()));
    }
    // 只查现场那一页不够：栈向下生长进入的深层页若与堆别名，仍会把返回地址写进堆。
    if !overlaps
        && let Some((low, high)) = super::stacks::window(cpu_index())
    {
        let mut page = low;
        while page < high {
            if crate::paging::translate_current(hk_hal::VirtAddr::new(page))
                .is_some_and(|phys| in_heap_phys(phys.get()))
            {
                overlaps = true;
                break;
            }
            page += hk_hal::PAGE_SIZE as u64;
        }
    }
    if !overlaps {
        return;
    }
    let mut report = Report(Serial::new(COM1));
    let _ = writeln!(
        report,
        "\n内核：中断栈落在内核堆内 cpu={} rsp={address:#018x} 堆虚拟={heap_virt:#018x}..{:#018x} 堆物理={heap_phys:#018x}..{:#018x} vector={:#x} cs={:#x}",
        cpu_index(),
        heap_virt.saturating_add(heap_len),
        heap_phys.saturating_add(heap_len),
        frame.vector,
        frame.cs
    );
    hk_hal::exit(hk_hal::ExitCode::Failure(73))
}
