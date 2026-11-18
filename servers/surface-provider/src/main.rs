//! Surface Provider：为显示表面提供零页，并接受能力释放通知。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, PROVIDER_ERROR, PROVIDER_EVICT, PROVIDER_FAULT, PROVIDER_KIND_SURFACE,
    PROVIDER_PROVIDE, PROVIDER_RELEASE, PROVIDER_WRITEBACK, capability_bind_provider,
    capability_create_lazy, capacity_alloc, close_handle, frame_write, message_words,
    recv_data_handle, reply, reply_handle, send_handle,
};

hk_server_api::server_heap!(64 * 1024);

/// 页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    let mut buffer = [0_u8; 32];
    loop {
        let (label, word, sender, _length, _handle) = recv_data_handle(endpoint, &mut buffer, 0);
        if label == hk_proto::display::SURFACE_ALLOC {
            serve_alloc(endpoint, sender, word);
        } else if label == PROVIDER_FAULT {
            serve_fault(capacity);
        } else if label == PROVIDER_RELEASE
            || label == PROVIDER_EVICT
            || label == PROVIDER_WRITEBACK
        {
            // 表面页没有持久化内容；释放/驱逐/回写只需确认。
        }
    }
}

/// 创建懒 Surface 能力并回传句柄。
#[cfg(target_os = "none")]
fn serve_alloc(endpoint: u64, sender: u64, bytes: u64) {
    let Some(bytes) = usize::try_from(bytes).ok().filter(|bytes| *bytes > 0) else {
        return;
    };
    let Some(capability) = capability_create_lazy(CapabilityKind::Surface, bytes) else {
        return;
    };
    if capability_bind_provider(capability, endpoint, PROVIDER_KIND_SURFACE) != 0 {
        let _ = close_handle(capability);
        return;
    }
    if send_handle(
        sender,
        hk_proto::display::SURFACE_ALLOCATED,
        bytes as u64,
        capability,
    ) != 0
    {
        let _ = close_handle(capability);
    }
}

/// 分配并回传一页零填充内容。
#[cfg(target_os = "none")]
fn serve_fault(capacity: u64) {
    let Some(words) = message_words() else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let length = (words[1] as usize).clamp(1, PAGE);
    let Some(frame) = capacity_alloc(capacity, length) else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let zeros = [0_u8; PAGE];
    if frame_write(frame, 0, &zeros[..length]) != 0 {
        let _ = close_handle(frame);
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    }
    let _ = reply_handle(PROVIDER_PROVIDE, 0, frame);
}

hk_server_api::server_boilerplate!();
