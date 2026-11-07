//! 中断与异常现场。

use alloc::vec::Vec;

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

/// 现场字段数。
const FRAME_WORDS: usize = 22;

impl InterruptFrame {
    /// 编码为 22 个小端 u64。
    pub fn encode(&self) -> Vec<u8> {
        let words = [
            self.rax,
            self.rbx,
            self.rcx,
            self.rdx,
            self.rsi,
            self.rdi,
            self.rbp,
            self.r8,
            self.r9,
            self.r10,
            self.r11,
            self.r12,
            self.r13,
            self.r14,
            self.r15,
            self.vector,
            self.error,
            self.rip,
            self.cs,
            self.rflags,
            self.rsp,
            self.ss,
        ];
        let mut out = Vec::with_capacity(FRAME_WORDS * 8);
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    /// 从 22 个小端 u64 解码。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != FRAME_WORDS * 8 {
            return None;
        }
        let mut words = [0_u64; FRAME_WORDS];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 8;
            *word = u64::from_le_bytes(bytes[start..start + 8].try_into().ok()?);
        }
        Some(Self {
            rax: words[0],
            rbx: words[1],
            rcx: words[2],
            rdx: words[3],
            rsi: words[4],
            rdi: words[5],
            rbp: words[6],
            r8: words[7],
            r9: words[8],
            r10: words[9],
            r11: words[10],
            r12: words[11],
            r13: words[12],
            r14: words[13],
            r15: words[14],
            vector: words[15],
            error: words[16],
            rip: words[17],
            cs: words[18],
            rflags: words[19],
            rsp: words[20],
            ss: words[21],
        })
    }
}
