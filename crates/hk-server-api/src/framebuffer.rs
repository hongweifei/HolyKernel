//! 帧缓冲访问协议。
//!
//! 只有显示服务可以查询并映射帧缓冲；内核按服务名授权，避免普通任务直接
//! 接触显示设备。

pub use hk_syscall::{FRAMEBUFFER_DESCRIPTOR_BYTES, FramebufferDescriptor};

/// 显示服务映射帧缓冲时使用的用户虚拟地址。
pub const MAP_ADDRESS: u64 = 0x7800_0000;

/// 查询帧缓冲描述；无帧缓冲或未授权时返回 `None`。
pub fn query() -> Option<FramebufferDescriptor> {
    let mut bytes = [0_u8; FRAMEBUFFER_DESCRIPTOR_BYTES];
    // SAFETY: 缓冲区属于当前进程；内核只写入描述符长度以内的字节。
    let length = unsafe {
        crate::syscall_call(
            crate::SYS_FRAMEBUFFER_INFO,
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

/// 把帧缓冲映射到 `virt`，返回映射字节数；失败返回 0。
pub fn map(virt: u64) -> u64 {
    // SAFETY: 内核校验调用者身份、地址对齐与映射范围。
    unsafe { crate::syscall_call(crate::SYS_FRAMEBUFFER_MAP, virt, 0, 0, 0).value }
}
