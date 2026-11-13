//! x86_64 上下文切换。
//!
//! 切换只保存 callee-saved 寄存器（rbp、rbx、r12-r15）与栈指针；返回地址留在
//! 各自栈上，恢复后由 `ret` 继续执行。

// `.type`/`.size` 是 ELF 专有伪指令，MSVC 与 Mach-O 汇编器不接受；符号本身
// 由 `.globl` + 标签定义，跨目标只需这一份指令序列。
core::arch::global_asm!(
    ".text",
    ".globl hk_swap_context",
    "hk_swap_context:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    ".globl hk_coroutine_start",
    "hk_coroutine_start:",
    "mov rdi, r12",
    "call hk_coroutine_entry",
    "ud2",
);

unsafe extern "C" {
    fn hk_swap_context(from: *mut u64, to: u64);
    fn hk_coroutine_start();
}

/// 返回协程入口的地址，写入初始栈作为返回目标。
pub(crate) fn entry_address() -> u64 {
    let entry: unsafe extern "C" fn() = hk_coroutine_start;
    entry as *const () as u64
}

/// 切换到目标栈；`from` 保存当前栈指针。
///
/// # Safety
/// 两个栈指针都必须指向有效栈；切换期间调用方的活跃局部变量在目标栈上不可见。
pub(crate) unsafe fn switch(from: *mut u64, to: u64) {
    // SAFETY: 调用方保证栈与现场有效；汇编只保存 callee-saved 寄存器。
    unsafe {
        hk_swap_context(from, to);
    }
}
