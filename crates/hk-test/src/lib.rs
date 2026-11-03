//! 测试支撑与退出协议。
//!
//! 裸机测试与 QEMU 冒烟测试需要一条确定的退出通道：
//!
//! | 平台 | 通道 |
//! | --- | --- |
//! | Hosted（Linux/macOS/Windows 进程） | 进程退出码 |
//! | x86_64 QEMU | `isa-debug-exit` 端口 `0xF4`，QEMU 退出码为 `(code << 1) \| 1` |
//! | AArch64 QEMU | semihosting `SYS_EXIT_EXTENDED` |
//!
//! `isa-debug-exit` 携带 8 位诊断码；semihosting 携带 `code` 子码。两者都以 0
//! 表示成功。

#![no_std]

#[cfg(not(target_os = "none"))]
extern crate std;

/// 执行结果对应的退出码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// 成功。
    Success,
    /// 失败，携带非零诊断码。
    Failure(u8),
}

impl ExitCode {
    /// 返回退出码数值，成功为 0。
    pub const fn value(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure(code) => code,
        }
    }

    /// 判断是否表示成功。
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

/// 按当前平台退出协议终止执行，不返回。
///
/// 退出码 0 表示成功；非零值原样传给宿主环境或 QEMU。
pub fn exit(code: ExitCode) -> ! {
    #[cfg(not(target_os = "none"))]
    {
        std::process::exit(i32::from(code.value()));
    }

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    {
        // SAFETY: 0xF4 是 QEMU isa-debug-exit 设备端口；写入后 QEMU 立即终止，
        // 控制流不再返回。QEMU 以 `(value << 1) | 1` 作为自身退出码，因此这里
        // 只写诊断码本身，成功为 0、宿主侧退出码 1。
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xf4u16,
                in("al") code.value(),
                options(noreturn, nomem, nostack),
            );
        }
    }

    #[cfg(all(target_os = "none", target_arch = "aarch64"))]
    {
        let block: [usize; 2] = [0x20026, usize::from(code.value())];
        // SAFETY: `hlt #0xf000` 触发 semihosting；`SYS_EXIT_EXTENDED`（0x20）读取
        // 退出块并终止目标，控制流不再返回。
        unsafe {
            core::arch::asm!(
                "hlt #0xf000",
                in("x0") 0x20usize,
                in("x1") block.as_ptr(),
                options(noreturn, nostack),
            );
        }
    }
}
