//! Image Server：接收 Loader 的段注册，按缺页从模块文件复制代码页。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    PROVIDER_ERROR, PROVIDER_FAULT, PROVIDER_PROVIDE, capacity_alloc, frame_write, message_words,
    module_read, recv_data_handle, reply, reply_handle, write,
};
#[cfg(target_os = "none")]
use hk_syscall::IMAGE_REGISTER;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;
/// 页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// 一个段来源。
#[cfg(target_os = "none")]
#[derive(Clone, Copy, Default)]
struct Segment {
    object: u64,
    module: u64,
    file_offset: u64,
    file_len: u64,
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    let mut segments: Vec<Segment> = Vec::new();
    let mut buffer = [0_u8; 8];
    write(b"hk-server-image: ready\n");
    loop {
        let (label, _word, sender, _length, _handle) = recv_data_handle(endpoint, &mut buffer, 0);
        if label == IMAGE_REGISTER && sender == 0 {
            if let Some(words) = message_words() {
                let segment = Segment {
                    object: words[0],
                    module: words[1],
                    file_offset: words[2],
                    file_len: words[3],
                };
                if let Some(slot) = segments
                    .iter_mut()
                    .find(|slot| slot.object == segment.object)
                {
                    *slot = segment;
                } else {
                    segments.push(segment);
                }
            }
        } else if label == PROVIDER_FAULT {
            serve_fault(capacity, &segments);
        }
    }
}

/// 提供一页模块内容。
#[cfg(target_os = "none")]
fn serve_fault(capacity: u64, segments: &[Segment]) {
    let Some(words) = message_words() else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let Some(segment) = segments.iter().find(|segment| segment.object == words[4]) else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let offset = words[0];
    let length = (words[1] as usize).min(PAGE);
    let mut page = vec![0_u8; PAGE];
    let available = segment.file_len.saturating_sub(offset);
    let file_bytes = usize::try_from(available).unwrap_or(usize::MAX).min(length);
    if file_bytes > 0 {
        let read = module_read(
            segment.module,
            segment.file_offset.saturating_add(offset),
            &mut page[..file_bytes],
        );
        if read != file_bytes as u64 {
            let _ = reply(PROVIDER_ERROR, 0);
            return;
        }
    }
    let Some(frame) = capacity_alloc(capacity, PAGE) else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    if frame_write(frame, 0, &page) != 0 {
        let _ = hk_server_api::close_handle(frame);
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    }
    let _ = reply_handle(PROVIDER_PROVIDE, 0, frame);
}

hk_server_api::server_boilerplate!();
