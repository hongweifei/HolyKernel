//! virtio-sound 服务：独占音频设备，独立于块/控制台/GPU 驱动服务。
//!
//! 播放是异步的：`start_tone` 只受理并提交前两个 10 ms 周期，服务在播放期间
//! 以 1 tick 的节拍推进 `poll`，播完向发起者投递 `SOUND_DONE` 并按队列启动
//! 下一段请求。播放期间服务会 sleep，因此系统关机判定不会被空转卡住。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, TONE_HZ, TONE_MILLIS, TONE_RATE, probe};
#[cfg(target_os = "none")]
use hk_proto::sound::{
    SOUND_DONE, SOUND_INFO, SOUND_INFO_REPLY, SOUND_PING, SOUND_PONG, SOUND_SHUTDOWN, SOUND_STATUS,
    SOUND_STATUS_REPLY, SOUND_TONE, SOUND_TONE_ACK,
};
#[cfg(target_os = "none")]
use hk_user_api::device::{DEVICE_CLASS_SOUND, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_user_api::{
    close_handle, device_info, exit, format::Line, recv_data_handle, recv_data_handle_try, send,
    send_data, service_endpoint, sleep_ticks, write,
};

// 驱动 crate 需要全局分配器；本服务自身只用固定队列。
hk_user_api::server_heap!();

/// 等待受理的播放请求。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct Request {
    owner: u64,
    hz: u32,
    millis: u32,
}

/// 待播请求队列：固定容量，服务不使用堆。
#[cfg(target_os = "none")]
const QUEUE_SLOTS: usize = 8;

#[cfg(target_os = "none")]
struct Queue {
    items: [Option<Request>; QUEUE_SLOTS],
    head: usize,
    len: usize,
}

#[cfg(target_os = "none")]
impl Queue {
    const fn new() -> Self {
        Self {
            items: [None; QUEUE_SLOTS],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, request: Request) -> bool {
        if self.len == QUEUE_SLOTS {
            return false;
        }
        let index = (self.head + self.len) % QUEUE_SLOTS;
        self.items[index] = Some(request);
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<Request> {
        if self.len == 0 {
            return None;
        }
        let request = self.items[self.head].take();
        self.head = (self.head + 1) % QUEUE_SLOTS;
        self.len -= 1;
        request
    }
}

/// 启动服务。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write(b"hk-server-virtio-sound: no device manager\n");
        exit(2)
    }
    let Some(grant) = request_grant(endpoint, manager) else {
        write(b"hk-server-virtio-sound: no sound device\n");
        exit(0)
    };
    let Some(Device::Sound(sound)) = probe_grant(grant) else {
        write(b"hk-server-virtio-sound: probe failed\n");
        exit(3)
    };
    Line::new()
        .text(b"hk-server-virtio-sound: ready streams ")
        .number(sound.output_streams().unwrap_or(0) as u64)
        .text(b" jacks ")
        .number(u64::from(sound.jacks()))
        .text(b"\n")
        .finish();

    // 启动自检：播 440 Hz 一段，结果计入 WAV。
    let mut queue = Queue::new();
    let _ = sound.start_tone(TONE_HZ, TONE_MILLIS, 0);

    let mut buffer = [0_u8; 256];
    loop {
        if sound.playing() > 0 {
            // 播放中：消息不阻塞处理，1 tick 后继续泵。
            while let Some(message) = try_message(endpoint, &mut buffer) {
                handle(&sound, &mut queue, message, &buffer);
            }
            complete(&sound, &mut queue, &mut buffer, endpoint);
            let _ = sleep_ticks(1);
        } else {
            let (label, word, sender, length, received) =
                recv_data_handle(endpoint, &mut buffer, 0);
            if received != 0 {
                let _ = close_handle(received);
            }
            let message = (label, word, sender, length.min(buffer.len()));
            handle(&sound, &mut queue, message, &buffer);
            complete(&sound, &mut queue, &mut buffer, endpoint);
        }
    }
}

/// 回收已完成的流：通知发起者，并把队列中的请求补到空闲流上。
#[cfg(target_os = "none")]
fn complete(sound: &hk_driver_virtio::Sound, queue: &mut Queue, buffer: &mut [u8], endpoint: u64) {
    while let Some((_stream, owner, ok)) = sound.poll() {
        if owner != 0 {
            let _ = send(owner, SOUND_DONE, u64::from(ok));
        }
        while let Some(request) = queue.pop() {
            if let Some((stream, rate)) =
                sound.start_tone(request.hz, request.millis, request.owner)
            {
                Line::new()
                    .text(b"hk-server-virtio-sound: start queued ")
                    .number(u64::from(request.hz))
                    .text(b" Hz stream ")
                    .number(u64::from(stream))
                    .text(b" rate ")
                    .number(u64::from(rate))
                    .text(b"\n")
                    .finish();
                break;
            }
            if request.owner != 0 {
                let _ = send(request.owner, SOUND_DONE, 0);
            }
        }
        while let Some(message) = try_message(endpoint, buffer) {
            handle(sound, queue, message, buffer);
        }
    }
}

/// 非阻塞取一条消息。
#[cfg(target_os = "none")]
fn try_message(endpoint: u64, buffer: &mut [u8]) -> Option<(u64, u64, u64, usize)> {
    let (label, word, sender, length, received) = recv_data_handle_try(endpoint, buffer, 0);
    if received != 0 {
        let _ = close_handle(received);
    }
    (label != 0).then_some((label, word, sender, length.min(buffer.len())))
}

/// 处理一条协议消息。
#[cfg(target_os = "none")]
fn handle(
    sound: &hk_driver_virtio::Sound,
    queue: &mut Queue,
    message: (u64, u64, u64, usize),
    buffer: &[u8],
) {
    let (label, word, sender, length) = message;
    let data = &buffer[..length];
    match label {
        SOUND_INFO => {
            let Some(streams) = sound.output_streams() else {
                return;
            };
            let mut payload = [0_u8; 16];
            payload[0..4].copy_from_slice(&(streams as u32).to_le_bytes());
            payload[4..8].copy_from_slice(&TONE_RATE.to_le_bytes());
            payload[8..12].copy_from_slice(&2_u32.to_le_bytes());
            payload[12..16].copy_from_slice(&sound.jacks().to_le_bytes());
            let _ = send_data(sender, SOUND_INFO_REPLY, &payload);
        }
        SOUND_TONE => {
            let _ = word;
            let hz = if data.len() >= 4 {
                u32::from_le_bytes(data[..4].try_into().unwrap_or([0; 4]))
            } else {
                0
            };
            let millis = if data.len() >= 8 {
                u32::from_le_bytes(data[4..8].try_into().unwrap_or([0; 4]))
            } else {
                0
            };
            let hz = if hz == 0 { TONE_HZ } else { hz };
            let millis = if millis == 0 { TONE_MILLIS } else { millis };
            // 有空闲流就直接开始；全部占用时排队，队列满才拒绝。
            let started = sound.start_tone(hz, millis, sender);
            if let Some((stream, rate)) = started {
                Line::new()
                    .text(b"hk-server-virtio-sound: start ")
                    .number(u64::from(hz))
                    .text(b" Hz stream ")
                    .number(u64::from(stream))
                    .text(b" rate ")
                    .number(u64::from(rate))
                    .text(b"\n")
                    .finish();
            }
            let accepted = started.is_some()
                || queue.push(Request {
                    owner: sender,
                    hz,
                    millis,
                });
            let _ = send(sender, SOUND_TONE_ACK, u64::from(accepted));
        }
        SOUND_STATUS => {
            let _ = send(sender, SOUND_STATUS_REPLY, sound.playing() as u64);
        }
        SOUND_PING => {
            let _ = send(sender, SOUND_PONG, 0);
        }
        SOUND_SHUTDOWN => exit(0),
        _ => {}
    }
}

/// 申请音频设备授权。
#[cfg(target_os = "none")]
fn request_grant(endpoint: u64, manager: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_SOUND);
    loop {
        let (label, _word, _sender, handle) = hk_user_api::recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
        if handle != 0 {
            let _ = close_handle(handle);
        }
    }
}

/// 读取授权描述并探测设备。
#[cfg(target_os = "none")]
fn probe_grant(handle: u64) -> Option<Device> {
    let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
    let length = device_info(handle, &mut bytes) as usize;
    if length == u64::MAX as usize {
        return None;
    }
    let descriptor = DeviceGrant::decode(&bytes[..length])?;
    probe(handle, descriptor).ok()
}

hk_user_api::server_boilerplate!();
