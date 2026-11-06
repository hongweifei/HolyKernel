//! 服务协议、客户端与运行时辅助。
//!
//! 本 crate 描述系统调用参数与返回约定、设备/网络/显示/控制台/输入等线协议，
//! 并提供服务发现与入口样板；具体入口指令由 [`arch`] 按架构提供。

#![no_std]

mod arch;
pub mod block;
pub mod checkpoint;
pub mod console;
pub mod device;
pub mod display;
pub mod format;
pub mod framebuffer;
pub mod heap;
pub mod holy;
pub mod input;
pub mod net;
pub mod root;
pub mod runtime;
pub mod socket;
pub mod vfs;

pub use hk_syscall::*;

/// 生成服务入口样板：内核态 panic 处理器与宿主占位 `main`。
///
/// 每个服务二进制在 crate 根部调用一次；`_start` 仍由服务自己实现。
#[macro_export]
macro_rules! server_boilerplate {
    () => {
        #[cfg(target_os = "none")]
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo) -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        #[cfg(not(target_os = "none"))]
        fn main() {
            eprintln!(concat!(
                env!("CARGO_PKG_NAME"),
                " 只在 x86_64-unknown-none 目标上构建"
            ));
        }
    };
}

pub(crate) use arch::call as syscall_call;

mod syscall;
pub use syscall::*;
