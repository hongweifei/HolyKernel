//! 块服务端循环：映射共享帧、提交异步请求并回复完成。

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use hk_block::AsyncBlockDevice;
use hk_proto::block::{
    BLOCK_ATTACH, BLOCK_DETACH, BLOCK_REQUEST, BLOCK_RESPONSE, BLOCK_SHUTDOWN, BUFFER_LEN,
    BUFFER_SLOTS, OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE, STATUS_BUSY, STATUS_INVALID, STATUS_OK,
    attach_length, attach_slot, decode_request, encode_response,
};
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, capability_phys, close_handle, handle_duplicate,
    recv_data_handle, recv_data_handle_try, region_info, region_map, region_unmap, send_data,
    yield_now,
};

use crate::status::to_status;

/// 服务端映射的一个共享缓冲区。
struct ServerMapping {
    virt: u64,
    region: u64,
    /// 客户端转移来的匿名能力句柄；解除映射时一并关闭。
    handle: u64,
    len: usize,
    busy: bool,
}

/// 等待完成的一条请求。
struct Pending {
    sender: u64,
    slot: u32,
}

/// 在端点上服务块设备请求，直到任务退出或收到退出请求。
pub fn serve(endpoint: u64, disk: &mut dyn AsyncBlockDevice) {
    serve_with_hook(endpoint, disk, |_, _, _, _| {});
}

/// 服务块请求，并在共享帧映射时通知驱动登记 DMA 区域。
pub fn serve_with_hook<F: FnMut(u64, u64, usize, u64)>(
    endpoint: u64,
    disk: &mut dyn AsyncBlockDevice,
    hook: F,
) {
    serve_with_wait(endpoint, disk, hook, || {
        yield_now();
    });
}

/// 同 [`serve_with_hook`]，但等待在途完成时不轮询：`wait` 返回后读取设备中断状态。
pub fn serve_with_wait<F, W>(endpoint: u64, disk: &mut dyn AsyncBlockDevice, hook: F, wait: W)
where
    F: FnMut(u64, u64, usize, u64),
    W: FnMut(),
{
    serve_with_wait_and(endpoint, disk, hook, wait, |_, _, _, _, _| {});
}

/// 同 [`serve_with_wait`]，并把非块协议消息交给 `other` 处理。
///
/// `other` 收到 `(label, word, sender, 数据, 随消息收到的句柄)`；句柄由回调负责
/// 关闭。块协议标签由本函数处理，不会传入回调。
pub fn serve_with_wait_and<F, W, O>(
    endpoint: u64,
    disk: &mut dyn AsyncBlockDevice,
    mut hook: F,
    mut wait: W,
    mut other: O,
) where
    F: FnMut(u64, u64, usize, u64),
    W: FnMut(),
    O: FnMut(u64, u64, u64, &[u8], u64),
{
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut response = vec![0_u8; 64 * 1024];
    let mut frames: BTreeMap<(u64, u32), ServerMapping> = BTreeMap::new();
    let mut inflight: BTreeMap<u64, Pending> = BTreeMap::new();

    loop {
        while let Some((token, result)) = disk.poll_complete() {
            let Some(pending) = inflight.remove(&token) else {
                continue;
            };
            if let Some(frame) = frames.get_mut(&(pending.sender, pending.slot)) {
                frame.busy = false;
            }
            let (status, written) = match result {
                Ok(written) => (STATUS_OK, written),
                Err(error) => (to_status(error), 0),
            };
            reply(
                pending.sender,
                pending.slot,
                status,
                &(written as u64).to_le_bytes(),
                &mut response,
            );
        }

        let (label, word, sender, length, received) = if inflight.is_empty() {
            recv_data_handle(endpoint, &mut buffer, 0)
        } else {
            let message = recv_data_handle_try(endpoint, &mut buffer, 0);
            if message.0 == 0 {
                wait();
                disk.ack_interrupt();
                continue;
            }
            message
        };

        if !is_block_label(label) {
            other(
                label,
                word,
                sender,
                &buffer[..length.min(buffer.len())],
                received,
            );
            continue;
        }
        if label == BLOCK_SHUTDOWN {
            return;
        }
        if label == BLOCK_ATTACH {
            let slot = attach_slot(word);
            let length = attach_length(word);
            if received == 0 || length == 0 || length > BUFFER_LEN || slot >= BUFFER_SLOTS {
                if received != 0 {
                    let _ = close_handle(received);
                }
                continue;
            }
            let key = (sender, slot);
            let previous = frames.remove(&key);
            if let Some(old) = &previous {
                let _ = region_unmap(old.region);
                let _ = close_handle(old.handle);
            }
            let Some((phys, capability_len)) = capability_phys(received) else {
                let _ = close_handle(received);
                continue;
            };
            if capability_len < length as u64 {
                let _ = close_handle(received);
                continue;
            }
            let duplicate = handle_duplicate(received, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
            if duplicate == u64::MAX {
                let _ = close_handle(received);
                continue;
            }
            let Some(region) = region_map(received, 0, length, RIGHT_READ | RIGHT_WRITE, 0) else {
                let _ = close_handle(received);
                let _ = close_handle(duplicate);
                continue;
            };
            let Some((virt, _)) = region_info(region) else {
                let _ = region_unmap(region);
                let _ = close_handle(received);
                let _ = close_handle(duplicate);
                continue;
            };
            hook(virt, phys, length, duplicate);
            let _ = close_handle(duplicate);
            frames.insert(
                key,
                ServerMapping {
                    virt,
                    region,
                    handle: received,
                    len: length,
                    busy: false,
                },
            );
            continue;
        }
        if label == BLOCK_DETACH {
            if word == u64::MAX {
                for (region, handle) in detach_sender(&mut frames, sender) {
                    let _ = region_unmap(region);
                    let _ = close_handle(handle);
                }
            } else if let Some(frame) = frames.remove(&(sender, word as u32)) {
                let _ = region_unmap(frame.region);
                let _ = close_handle(frame.handle);
            }
            continue;
        }
        if label != BLOCK_REQUEST {
            continue;
        }
        let Some(request) = decode_request(&buffer[..length.min(buffer.len())]) else {
            reply(sender, 0, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let Some(frame) = frames.get(&(sender, request.tag)) else {
            match request.op {
                OP_CAPACITY => {
                    reply(
                        sender,
                        request.tag,
                        STATUS_OK,
                        &disk.capacity_bytes().to_le_bytes(),
                        &mut response,
                    );
                }
                OP_FLUSH => {
                    let status = match disk.flush() {
                        Ok(()) => STATUS_OK,
                        Err(error) => to_status(error),
                    };
                    reply(sender, request.tag, status, &[], &mut response);
                }
                _ => reply(sender, request.tag, STATUS_INVALID, &[], &mut response),
            }
            continue;
        };
        if frame.busy {
            reply(sender, request.tag, STATUS_BUSY, &[], &mut response);
            continue;
        }
        let (virt, frame_len) = (frame.virt, frame.len);
        let result = match request.op {
            OP_READ | OP_WRITE => {
                let length = request.length as usize;
                if length == 0 || length > frame_len {
                    reply(sender, request.tag, STATUS_INVALID, &[], &mut response);
                    continue;
                }
                // SAFETY: 帧在客户端持有期间保持映射；服务端单线程，完成前不再访问。
                let data = unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, frame_len) };
                match request.op {
                    OP_READ => disk.submit_read(request.block, &mut data[..length]),
                    _ => disk.submit_write(request.block, &data[..length]),
                }
            }
            OP_FLUSH => match disk.flush() {
                Ok(()) => {
                    reply(
                        sender,
                        request.tag,
                        STATUS_OK,
                        &0u64.to_le_bytes(),
                        &mut response,
                    );
                    continue;
                }
                Err(error) => Err(error),
            },
            OP_CAPACITY => {
                reply(
                    sender,
                    request.tag,
                    STATUS_OK,
                    &disk.capacity_bytes().to_le_bytes(),
                    &mut response,
                );
                continue;
            }
            _ => {
                reply(sender, request.tag, STATUS_INVALID, &[], &mut response);
                continue;
            }
        };
        match result {
            Ok(token) => {
                if let Some(frame) = frames.get_mut(&(sender, request.tag)) {
                    frame.busy = true;
                }
                inflight.insert(
                    token,
                    Pending {
                        sender,
                        slot: request.tag,
                    },
                );
            }
            Err(error) => {
                reply(sender, request.tag, to_status(error), &[], &mut response);
            }
        }
    }
}

/// 判断标签是否属于块协议。
fn is_block_label(label: u64) -> bool {
    matches!(
        label,
        BLOCK_SHUTDOWN | BLOCK_ATTACH | BLOCK_DETACH | BLOCK_REQUEST
    )
}

fn detach_sender(frames: &mut BTreeMap<(u64, u32), ServerMapping>, sender: u64) -> Vec<(u64, u64)> {
    let keys: Vec<(u64, u32)> = frames
        .keys()
        .filter(|(owner, _)| *owner == sender)
        .copied()
        .collect();
    let mut removed = Vec::new();
    for key in keys {
        if let Some(frame) = frames.remove(&key) {
            removed.push((frame.region, frame.handle));
        }
    }
    removed
}

fn reply(sender: u64, tag: u32, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, tag, status, data) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_block_labels() {
        for label in [BLOCK_SHUTDOWN, BLOCK_ATTACH, BLOCK_DETACH, BLOCK_REQUEST] {
            assert!(is_block_label(label));
        }
        assert!(!is_block_label(0));
        assert!(!is_block_label(BLOCK_RESPONSE));
    }

    #[test]
    fn detaches_all_frames_of_one_sender() {
        let mut frames = BTreeMap::new();
        for (owner, slot, region) in [(7, 0, 11), (7, 1, 22), (8, 0, 33)] {
            frames.insert(
                (owner, slot),
                ServerMapping {
                    virt: 0x1000 + region,
                    region,
                    handle: region + 100,
                    len: 512,
                    busy: false,
                },
            );
        }
        assert_eq!(detach_sender(&mut frames, 7), vec![(11, 111), (22, 122)]);
        assert_eq!(frames.len(), 1);
        assert!(frames.contains_key(&(8, 0)));
        assert!(detach_sender(&mut frames, 9).is_empty());
    }
}
