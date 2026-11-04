//! 中断与异常现场。

/// 中断与异常现场。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    /// RAX。
    pub rax: u64,
    /// RBX。
    pub rbx: u64,
    /// RCX。
    pub rcx: u64,
    /// RDX。
    pub rdx: u64,
    /// RSI。
    pub rsi: u64,
    /// RDI。
    pub rdi: u64,
    /// RBP。
    pub rbp: u64,
    /// R8。
    pub r8: u64,
    /// R9。
    pub r9: u64,
    /// R10。
    pub r10: u64,
    /// R11。
    pub r11: u64,
    /// R12。
    pub r12: u64,
    /// R13。
    pub r13: u64,
    /// R14。
    pub r14: u64,
    /// R15。
    pub r15: u64,
    /// 中断向量。
    pub vector: u64,
    /// 错误码。
    pub error: u64,
    /// 出错指令地址。
    pub rip: u64,
    /// 代码段选择子。
    pub cs: u64,
    /// 标志寄存器。
    pub rflags: u64,
    /// 中断发生时的栈指针。
    pub rsp: u64,
    /// 栈段选择子。
    pub ss: u64,
}

const _: () = {
    assert!(core::mem::offset_of!(InterruptFrame, rax) == 0);
    assert!(core::mem::offset_of!(InterruptFrame, rbx) == 8);
    assert!(core::mem::offset_of!(InterruptFrame, rcx) == 16);
    assert!(core::mem::offset_of!(InterruptFrame, rdx) == 24);
    assert!(core::mem::offset_of!(InterruptFrame, rsi) == 32);
    assert!(core::mem::offset_of!(InterruptFrame, rdi) == 40);
    assert!(core::mem::offset_of!(InterruptFrame, rbp) == 48);
    assert!(core::mem::offset_of!(InterruptFrame, r8) == 56);
    assert!(core::mem::offset_of!(InterruptFrame, r15) == 112);
    assert!(core::mem::offset_of!(InterruptFrame, vector) == 120);
    assert!(core::mem::offset_of!(InterruptFrame, error) == 128);
    assert!(core::mem::offset_of!(InterruptFrame, rip) == 136);
    assert!(core::mem::offset_of!(InterruptFrame, cs) == 144);
    assert!(core::mem::offset_of!(InterruptFrame, rflags) == 152);
    assert!(core::mem::offset_of!(InterruptFrame, rsp) == 160);
    assert!(core::mem::offset_of!(InterruptFrame, ss) == 168);
};
