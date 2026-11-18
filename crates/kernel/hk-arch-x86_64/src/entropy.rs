//! x86_64 熵源：优先 RDRAND，缺失时退化为 TSC 混洗。
//!
//! 退化路径只用于没有 RDRAND 的 CPU，不承诺密码学强度；内核不为熵源提供
//! 系统调用，平台 `Entropy` 供内部初始化与未来的用户态服务使用。

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::Entropy;

/// 全局熵源。
pub static ENTROPY: X86Entropy = X86Entropy;

/// x86_64 熵源。
pub struct X86Entropy;

static FALLBACK_STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

impl X86Entropy {
    fn rdrand() -> Option<u64> {
        if !crate::cpu::Features::detect().rdrand {
            return None;
        }
        for _ in 0..10 {
            let value: u64;
            let ok: u8;
            // SAFETY: CPUID 已确认支持 RDRAND；指令只写目标寄存器与进位标志。
            unsafe {
                asm!(
                    "rdrand {value}",
                    "setc {ok}",
                    value = out(reg) value,
                    ok = out(reg_byte) ok,
                    options(nostack, preserves_flags),
                );
            }
            if ok != 0 {
                return Some(value);
            }
        }
        None
    }

    fn fallback() -> u64 {
        let counter = FALLBACK_STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        let mut value = counter ^ crate::timing::rdtsc();
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}

impl Entropy for X86Entropy {
    fn fill(&self, buffer: &mut [u8]) {
        for chunk in buffer.chunks_mut(8) {
            let value = Self::rdrand().unwrap_or_else(Self::fallback);
            let bytes = value.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}
