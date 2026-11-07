//! 内核提供给用户态服务的通用面。
//!
//! 本 crate 描述系统调用参数与返回约定、HMP 能力与 Region 封装、Root 控制、
//! 设备授权与运行时样板；服务线协议与固定缓冲客户端在 `hk-proto`。

#![no_std]

mod arch;
pub mod device;
pub mod format;
pub mod framebuffer;
pub mod heap;
pub mod memory;
pub mod memory_manager;
pub mod root;
pub mod runtime;
pub mod syscall;

pub use hk_syscall::{
    CapabilityKind, DISPLAY_SERVICE, FAULT_EVENT, RIGHT_ADMIN, RIGHT_EXECUTE, RIGHT_READ,
    RIGHT_TRANSFER, RIGHT_WRITE,
};

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

pub use memory::*;
pub use memory_manager::*;
pub use syscall::*;
