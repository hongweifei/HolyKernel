//! 内核提供给用户态服务的通用面。
//!
//! 本 crate 描述 HMP 能力与 Region 封装、设备授权、帧缓冲与运行时样板；
//! 系统调用薄封装由 `hk-syscall-client` 提供并在此重新导出，服务线协议与
//! 固定缓冲客户端在 `hk-proto`。

#![no_std]

pub mod device;
pub mod format;
pub mod framebuffer;
pub mod heap;
pub mod memory;
pub mod memory_manager;
pub mod runtime;

pub use hk_syscall::{
    CapabilityKind, DISPLAY_SERVICE, FAULT_EVENT, RIGHT_ADMIN, RIGHT_EXECUTE, RIGHT_READ,
    RIGHT_TRANSFER, RIGHT_WRITE,
};

/// panic 输出写入器；panic 处理器通过它在不分配内存的情况下写串口。
pub struct PanicWriter;

impl core::fmt::Write for PanicWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        crate::write(text.as_bytes());
        Ok(())
    }
}

/// 生成服务入口样板：内核态 panic 处理器与宿主占位 `main`。
///
/// 每个服务二进制在 crate 根部调用一次；`_start` 仍由服务自己实现。
#[macro_export]
macro_rules! server_boilerplate {
    () => {
        #[cfg(target_os = "none")]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            let mut writer = $crate::PanicWriter;
            let _ = core::fmt::write(
                &mut writer,
                format_args!(
                    "\nhk-server panic: {} [{}]\n",
                    env!("CARGO_PKG_NAME"),
                    info.message()
                ),
            );
            if let Some(location) = info.location() {
                let _ = core::fmt::write(
                    &mut writer,
                    format_args!(
                        "  at {}:{}:{}\n",
                        location.file(),
                        location.line(),
                        location.column()
                    ),
                );
            }
            $crate::exit(1)
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

pub(crate) use hk_syscall_client::call as syscall_call;

pub use hk_syscall_client::syscall::*;
pub use hk_syscall_client::{root, syscall};
pub use memory::*;
pub use memory_manager::*;
