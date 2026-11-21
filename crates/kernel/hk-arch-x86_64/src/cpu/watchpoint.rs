//! 调试期写入监视：用 DR0 记录一次任务条目尾部的写入现场。
//!
//! 任务条目尾部被踩后只能晚到 `process()` 才暴露，无法指出写入者。这里把
//! `tcb` 字段的地址交给 DR0，任何写该 8 字节的指令都会触发 #DB；处理器保存的
//! RIP 就是写入指令（数据断点是 trap，指向触发指令之后），符号化即可定位。

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hk_hal::{Console, MAX_CPUS};

use super::InterruptFrame;
use crate::serial::{COM1, Serial};

/// 要监视写入的 8 字节地址；0 表示未启用。
static WATCH: AtomicU64 = AtomicU64::new(0);
/// 已输出的命中次数；写入循环只记前几次，避免刷屏。
static REPORTED: AtomicUsize = AtomicUsize::new(0);
/// 命中次数（含未打印的）；panic 处理器用它判断监视是否触发过。
static HITS: AtomicUsize = AtomicUsize::new(0);
/// 最近一次命中的 `(RIP, 监视地址, DR6, rsp 返回地址, rbp 返回地址)`。
static LAST_RIP: AtomicU64 = AtomicU64::new(0);
static LAST_ADDR: AtomicU64 = AtomicU64::new(0);
static LAST_DR6: AtomicU64 = AtomicU64::new(0);
static LAST_RSP_RET: AtomicU64 = AtomicU64::new(0);
static LAST_RBP_RET: AtomicU64 = AtomicU64::new(0);
static LAST_CPU: AtomicU64 = AtomicU64::new(0);
static LAST_CS: AtomicU64 = AtomicU64::new(0);
static LAST_WRSP: AtomicU64 = AtomicU64::new(0);
static LAST_WRBP: AtomicU64 = AtomicU64::new(0);
/// 沿帧指针的更深两层返回地址，用于定位 BTreeMap 操作的调用者。
static LAST_CHAIN1: AtomicU64 = AtomicU64::new(0);
static LAST_CHAIN2: AtomicU64 = AtomicU64::new(0);
/// 每个 CPU 正在处理的系统调用号；写监视命中时一并报告，用来对上 handler。
static CUR_SYSCALL: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// 每个 CPU 当前进入的内核入口编辑（1=dispatch、2=step、3=preempt、4=finish、5=tasks）
/// 与辅助值；由运行时在每次入口时写入。
static CUR_ENTRY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static CUR_ENTRY_AUX: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// 记录本 CPU 正在处理的系统调用号。
pub(crate) fn note_syscall(cpu: usize, number: u64) {
    if let Some(slot) = CUR_SYSCALL.get(cpu) {
        slot.store(number, Ordering::Release);
    }
}

/// 记录本 CPU 当前所处的内核入口。
pub fn note_entry(cpu: usize, tag: u64, aux: u64) {
    if let Some(slot) = CUR_ENTRY.get(cpu) {
        slot.store(tag, Ordering::Release);
    }
    if let Some(slot) = CUR_ENTRY_AUX.get(cpu) {
        slot.store(aux, Ordering::Release);
    }
}

fn current_entry() -> (u64, u64) {
    let cpu = super::cpu_index();
    (
        CUR_ENTRY.get(cpu).map_or(0, |slot| slot.load(Ordering::Acquire)),
        CUR_ENTRY_AUX
            .get(cpu)
            .map_or(0, |slot| slot.load(Ordering::Acquire)),
    )
}

/// 返回本 CPU 最近一次进入的系统调用号。
fn current_syscall() -> u64 {
    CUR_SYSCALL
        .get(super::cpu_index())
        .map_or(0, |slot| slot.load(Ordering::Acquire))
}

/// 最多输出多少次命中。
const REPORT_LIMIT: usize = 8;

/// 返回最近一次写监视命中：
/// `(RIP, 地址, DR6, CS, RSP, RBP, rsp 返回, rbp 返回, CPU, 链1, 链2)`。
#[allow(clippy::type_complexity)]
pub fn last_hit() -> Option<(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64)> {
    (HITS.load(Ordering::Acquire) != 0).then(|| {
        (
            LAST_RIP.load(Ordering::Acquire),
            LAST_ADDR.load(Ordering::Acquire),
            LAST_DR6.load(Ordering::Acquire),
            LAST_CS.load(Ordering::Acquire),
            LAST_WRSP.load(Ordering::Acquire),
            LAST_WRBP.load(Ordering::Acquire),
            LAST_RSP_RET.load(Ordering::Acquire),
            LAST_RBP_RET.load(Ordering::Acquire),
            LAST_CPU.load(Ordering::Acquire),
            LAST_CHAIN1.load(Ordering::Acquire),
            LAST_CHAIN2.load(Ordering::Acquire),
        )
    })
}

/// 设置监视地址；每个 CPU 在下次中断入口同步到自己的 DR0。
pub fn watch_word(address: u64) {
    WATCH.store(address, Ordering::Release);
}

/// 当前监视地址；0 表示未启用。
pub fn watched_word() -> u64 {
    WATCH.load(Ordering::Acquire)
}

fn read_dr0() -> u64 {
    let value: u64;
    // SAFETY: 内核态读取自身调试寄存器。
    unsafe {
        core::arch::asm!("mov {}, dr0", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_dr0(value: u64) {
    // SAFETY: 内核态写入自身调试寄存器。
    unsafe {
        core::arch::asm!("mov dr0, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

fn read_dr6() -> u64 {
    let value: u64;
    // SAFETY: 内核态读取自身调试寄存器。
    unsafe {
        core::arch::asm!("mov {}, dr6", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_dr6(value: u64) {
    // SAFETY: 内核态写入自身调试寄存器。
    unsafe {
        core::arch::asm!("mov dr6, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

fn write_dr7(value: u64) {
    // SAFETY: 内核态写入自身调试寄存器。
    unsafe {
        core::arch::asm!("mov dr7, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

/// 让本 CPU 的 DR0/DR7 与全局监视地址一致。
pub(crate) fn sync() {
    let address = watched_word();
    if read_dr0() == address {
        return;
    }
    // 先关断点，改地址后再按“写 8 字节”启用：L0=1、RW0=写、LEN0=8。
    write_dr7(0);
    write_dr6(0);
    write_dr0(address);
    if address != 0 {
        // 8 字节长度编码需要 CR4.DE（调试扩展）。
        let mut cr4: u64;
        // SAFETY: 只读写本 CPU 的 CR4.DE 位。
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
            if cr4 & (1 << 3) == 0 {
                core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1 << 3), options(nomem, nostack, preserves_flags));
            }
        }
        write_dr7(0x000D_0001);
    }
}

struct Report(Serial);

impl core::fmt::Write for Report {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0.write_str(text);
        Ok(())
    }
}

/// 内核栈窗口或内核镜像内的地址可以安全读取。
fn readable(address: u64) -> bool {
    let in_image = (0xffff_ffff_8000_0000..0xffff_ffff_c000_0000).contains(&address);
    let in_stack = super::stacks::window(super::cpu_index())
        .is_some_and(|(low, high)| address >= low && address.saturating_add(8) <= high);
    in_image || in_stack
}

fn read_word(address: u64) -> u64 {
    if address == 0 || !address.is_multiple_of(8) || !readable(address) {
        return 0;
    }
    // SAFETY: 地址落在已映射的内核镜像或当前 CPU 栈窗口内。
    unsafe { core::ptr::read_unaligned(address as *const u64) }
}

fn is_code(address: u64) -> bool {
    // 内核镜像从 0xffffffff80000000 起，物理装载长度约 16 MiB；上界必须覆盖
    // 整个镜像，否则高地址段的调用者会被误判为“非代码”而跳过。
    (0xffff_ffff_8000_0000..0xffff_ffff_8100_0000).contains(&address)
}

/// 处理 #DB：命中 DR0 写监视时打印现场并继续执行。
///
/// 返回 `true` 表示本次异常已被消费（调用方直接返回）。数据断点是 trap，
/// 保存的 RIP 已经指向触发指令之后，因此清 DR6 后 `iretq` 可以安全继续。
pub(crate) fn report(frame: &InterruptFrame) -> bool {
    let dr6 = read_dr6();
    if dr6 & 1 == 0 {
        return false;
    }
    write_dr6(0);
    let address = read_dr0();
    // 触发指令通常是 memcpy 内部（compiler-builtins 没有帧指针），沿着被打断的
    // rsp 往上找第一个像内核代码的返回地址；同时记录 rbp+8 作为交叉验证。
    let mut rsp_ret = 0;
    // 原始栈顶：memcpy 是叶子函数，`[rsp]` 就是调用者返回地址；直接打出来，
    // 不依赖任何启发式判断。
    let mut raw = [0_u64; 6];
    for (index, slot) in raw.iter_mut().enumerate() {
        *slot = read_word(frame.rsp.wrapping_add(index as u64 * 8));
    }
    for step in 0..8_u64 {
        let word = read_word(frame.rsp.wrapping_add(step * 8));
        if is_code(word) {
            rsp_ret = word;
            break;
        }
    }
    let rbp_ret = read_word(frame.rbp.wrapping_add(8));
    let rbp_ret = if is_code(rbp_ret) { rbp_ret } else { 0 };
    // 沿帧指针继续往上走，打印更外层的调用者。
    let mut chain = [0_u64; 4];
    let mut frame_pointer = frame.rbp;
    for slot in chain.iter_mut() {
        if frame_pointer == 0 || !frame_pointer.is_multiple_of(8) || !readable(frame_pointer) {
            break;
        }
        *slot = read_word(frame_pointer.wrapping_add(8));
        let next = read_word(frame_pointer);
        if next <= frame_pointer {
            break;
        }
        frame_pointer = next;
    }
    LAST_CHAIN1.store(chain[1], Ordering::Release);
    LAST_CHAIN2.store(chain[2], Ordering::Release);
    LAST_RIP.store(frame.rip, Ordering::Release);
    LAST_ADDR.store(address, Ordering::Release);
    LAST_DR6.store(dr6, Ordering::Release);
    LAST_RSP_RET.store(rsp_ret, Ordering::Release);
    LAST_RBP_RET.store(rbp_ret, Ordering::Release);
    LAST_CPU.store(super::cpu_index() as u64, Ordering::Release);
    LAST_CS.store(frame.cs, Ordering::Release);
    LAST_WRSP.store(frame.rsp, Ordering::Release);
    LAST_WRBP.store(frame.rbp, Ordering::Release);
    HITS.fetch_add(1, Ordering::Release);
    let count = REPORTED.fetch_add(1, Ordering::Relaxed);
    if count >= REPORT_LIMIT {
        return true;
    }
    let mut report = Report(Serial::new(COM1));
    let (entry, entry_aux) = current_entry();
    let _ = writeln!(
        report,
        "\n[watch] 写入监视命中 #{count} cpu={} sysno={:#x} entry={entry}/0x{entry_aux:x} cs={:#x} rip={:#018x} rsp={:#018x} rbp={:#018x} rsp_ret={rsp_ret:#018x} rbp_ret={rbp_ret:#018x} chain={:#x},{:#x},{:#x} stack={:#x},{:#x},{:#x},{:#x},{:#x},{:#x} regs=rsi:{:#x},rdi:{:#x},rcx:{:#x},rdx:{:#x},rax:{:#x},rbx:{:#x},r14:{:#x},r15:{:#x} dr6={dr6:#x} 地址={address:#018x}",
        super::cpu_index(),
        current_syscall(),
        frame.cs,
        frame.rip,
        frame.rsp,
        frame.rbp,
        chain[0],
        chain[1],
        chain[2],
        raw[0],
        raw[1],
        raw[2],
        raw[3],
        raw[4],
        raw[5],
        frame.rsi,
        frame.rdi,
        frame.rcx,
        frame.rdx,
        frame.rax,
        frame.rbx,
        frame.r14,
        frame.r15
    );
    true
}
