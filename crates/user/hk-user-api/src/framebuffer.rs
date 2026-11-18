//! 帧缓冲访问协议。
//!
//! 只有显示服务可以查询并映射帧缓冲；内核按服务名授权，避免普通任务直接
//! 接触显示设备。

pub use hk_syscall::{FRAMEBUFFER_DESCRIPTOR_BYTES, FramebufferDescriptor};

use hk_syscall::{SYS_FRAMEBUFFER_INFO, SYS_FRAMEBUFFER_MAP};

/// 把帧缓冲映射到内核选择的虚拟地址，返回映射字节数与地址。
pub fn map() -> Option<(u64, u64)> {
    let mut virt = 0_u64;
    // SAFETY: `virt` 位于本栈帧；内核校验调用者身份后写入映射地址。
    let length = unsafe {
        crate::syscall_call(SYS_FRAMEBUFFER_MAP, &mut virt as *mut u64 as u64, 0, 0, 0).value
    };
    (length != 0 && virt != 0).then_some((length, virt))
}

/// 查询帧缓冲描述；无帧缓冲或未授权时返回 `None`。
pub fn query() -> Option<FramebufferDescriptor> {
    let mut bytes = [0_u8; FRAMEBUFFER_DESCRIPTOR_BYTES];
    // SAFETY: 缓冲区属于当前进程；内核只写入描述符长度以内的字节。
    let length = unsafe {
        crate::syscall_call(
            SYS_FRAMEBUFFER_INFO,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if length as usize != FRAMEBUFFER_DESCRIPTOR_BYTES {
        return None;
    }
    FramebufferDescriptor::decode(&bytes)
}
