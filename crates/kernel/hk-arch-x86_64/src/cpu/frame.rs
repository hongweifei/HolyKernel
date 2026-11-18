//! 中断与异常现场。

use alloc::vec::Vec;

/// XSAVE 区域字节数：x87/SSE 的 512 字节 legacy 区 + XSAVE 头 64 字节。
pub const FPU_BYTES: usize = 832;

/// 系统调用/中断栈顶为中断帧与 FPU 现场预留的空间。
///
/// 入口固定压入 176 字节，`InterruptFrame::fpu` 位于 `rsp + 192`；
/// AVX 的 XSAVE 区域为 832 字节，栈顶需低于栈尾 848 字节。
pub const STACK_RESERVE: usize = 192 + FPU_BYTES - 176;

/// x87/SSE/AVX 状态，布局与 `XSAVE`（标准格式）一致。
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct FpuState {
    bytes: [u8; FPU_BYTES],
}

impl FpuState {
    /// 返回与 `FINIT` 等价的初始状态。
    ///
    /// x87 控制字 0x037F 与 MXCSR 0x1F80 屏蔽全部异常；其余寄存器清空。
    /// 同时把 `XSTATE_BV` 置为当前 XCR0 的启用项，避免首跑继承旧值。
    pub fn reset() -> Self {
        let mut bytes = [0_u8; FPU_BYTES];
        bytes[0..2].copy_from_slice(&0x037F_u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&0x1F80_u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        let xstate = crate::cpu::fpu_xstate_mask() & 0x7;
        bytes[512..520].copy_from_slice(&xstate.to_le_bytes());
        Self { bytes }
    }

    /// 返回原始字节。
    pub fn as_bytes(&self) -> &[u8; FPU_BYTES] {
        &self.bytes
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            bytes: bytes.try_into().ok()?,
        })
    }
}

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
    /// x87/SSE 状态。
    pub fpu: FpuState,
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
    assert!(core::mem::offset_of!(InterruptFrame, fpu) == 192);
    assert!(core::mem::size_of::<InterruptFrame>() == 192 + FPU_BYTES);
};

/// 现场字段数。
const FRAME_WORDS: usize = 22;

impl InterruptFrame {
    /// 序列化后的字节数。
    pub const ENCODED_BYTES: usize = FRAME_WORDS * 8 + FPU_BYTES;

    /// 编码为 22 个小端 u64 后追加 FXSAVE 区域。
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
        let mut out = Vec::with_capacity(Self::ENCODED_BYTES);
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(self.fpu.as_bytes());
        out
    }

    /// 从 22 个小端 u64 与 FXSAVE 区域解码。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::ENCODED_BYTES {
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
            fpu: FpuState::from_bytes(&bytes[FRAME_WORDS * 8..])?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::user::user_frame;

    #[test]
    fn frame_round_trip_preserves_fpu_state() {
        let frame = user_frame(0x1000, 0x2000, 7);
        let bytes = frame.encode();
        assert_eq!(bytes.len(), InterruptFrame::ENCODED_BYTES);

        let decoded = InterruptFrame::decode(&bytes).expect("现场可解码");
        assert_eq!(decoded.rip, 0x1000);
        assert_eq!(decoded.rsp, 0x2000);
        assert_eq!(decoded.rdi, 7);
        assert_eq!(decoded.fpu.as_bytes(), frame.fpu.as_bytes());
    }

    #[test]
    fn rejects_truncated_frame() {
        assert!(InterruptFrame::decode(&[0_u8; 8]).is_none());
        assert!(InterruptFrame::decode(&[0_u8; InterruptFrame::ENCODED_BYTES - 1]).is_none());
    }

    #[test]
    fn reset_masks_fpu_exceptions() {
        let state = FpuState::reset();
        let bytes = state.as_bytes();
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x037F);
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            0x1F80
        );
    }
}
