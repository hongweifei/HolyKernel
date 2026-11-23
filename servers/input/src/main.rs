//! input：用户态 virtio-input 驱动。
//!
//! 向 Device Manager 逐个申请输入设备授权（键盘、平板等），把按键事件解码为
//! 字节序列、把绝对坐标与滚轮增量转发给显示服务；显示服务按焦点路由到窗口
//! 客户端。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::string::ToString;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, Input, probe};
#[cfg(target_os = "none")]
use hk_proto::input::{
    INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, INPUT_WHEEL, pack_pointer, pack_wheel,
};
#[cfg(target_os = "none")]
use hk_user_api::device::{DEVICE_CLASS_INPUT, DEVICE_GRANT, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_user_api::{
    device_info, exit, irq_bind, recv_handle, self_notification, send, service_endpoint,
    wait_notify, write, write_u64,
};

#[cfg(target_os = "none")]
mod keymap;

#[cfg(target_os = "none")]
use keymap::{
    ABS_MAX, ABS_X, ABS_Y, BTN_LEFT, BTN_TOUCH, EV_ABS, EV_KEY, EV_REL, KEY_LEFTALT, KEY_LEFTCTRL,
    KEY_LEFTSHIFT, KEY_RIGHTALT, KEY_RIGHTCTRL, KEY_RIGHTSHIFT, REL_WHEEL, key_bytes,
};

hk_user_api::server_heap!(512 * 1024);

/// 最多接管的输入设备数。
#[cfg(target_os = "none")]
const MAX_INPUTS: usize = 4;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write(b"hk-server-input: no device manager\n");
        exit(2)
    }

    let notification = self_notification();
    let mut inputs: Vec<Box<Input>> = Vec::new();
    let mut index = 0_u64;
    while inputs.len() < MAX_INPUTS {
        // 高 32 位为同类设备索引，低 32 位为类别。
        let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_INPUT | (index << 32));
        let (label, _word, _sender, handle) = recv_handle(endpoint);
        if label != DEVICE_GRANT || handle == 0 {
            break;
        }
        index += 1;

        let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
        let length = device_info(handle, &mut bytes) as usize;
        if length == u64::MAX as usize {
            continue;
        }
        let Some(descriptor) = DeviceGrant::decode(&bytes[..length]) else {
            continue;
        };
        let line = descriptor.irq.unwrap_or(0xFF);
        match probe(handle, descriptor) {
            Ok(Device::Input(input)) => {
                if irq_bind(handle, notification, 1) != 0 {
                    write(b"hk-server-input: irq bind failed\n");
                    continue;
                }
                write(b"hk-server-input: device ");
                write_u64(index - 1);
                write(b" line ");
                write_u64(u64::from(line));
                write(b"\n");
                inputs.push(input);
            }
            Ok(_) => {}
            Err(error) => {
                write(b"hk-server-input: ");
                write(error.to_string().as_bytes());
                write(b"\n");
            }
        }
    }

    // 本地复现/抓帧用的指针回放开关：默认关闭，需要时临时改为 true 再构建。
    const REPLAY_POINTER: bool = false;
    #[cfg(target_os = "none")]
    if REPLAY_POINTER {
        replay_drag();
    }

    if inputs.is_empty() {
        write(b"hk-server-input: no input device\n");
        exit(0)
    }
    write(b"hk-server-input: ready\n");
    write(b"hk-server-input: devices ");
    write_u64(inputs.len() as u64);
    write(b"\n");

    let mut shift = false;
    let mut ctrl = false;
    let mut alt = false;
    let mut abs_x = 0_u16;
    let mut abs_y = 0_u16;
    // 一次唤醒内的 X/Y 轴事件合并成一条指针消息：virtio-tablet 每次移动会先报
    // X 再报 Y，逐事件转发会让 Display 用半个新坐标重复合成一次。
    let mut pointer_pending = false;
    let mut wheel_pending = 0_i32;
    loop {
        for input in &mut inputs {
            input.ack_interrupt();
            while let Some(event) = input.pop_event() {
                match event.event_type {
                    EV_ABS => match event.code {
                        ABS_X => {
                            abs_x = event.value.min(ABS_MAX) as u16;
                            pointer_pending = true;
                        }
                        ABS_Y => {
                            abs_y = event.value.min(ABS_MAX) as u16;
                            pointer_pending = true;
                        }
                        _ => {}
                    },
                    EV_REL => {
                        if event.code == REL_WHEEL && event.value != 0 {
                            wheel_pending = wheel_pending.saturating_add(event.value as i32);
                        }
                    }
                    EV_KEY => {
                        if event.code == BTN_LEFT || event.code == BTN_TOUCH {
                            // 按钮按本批最新的指针位置命中，不能等到批次结束再补发。
                            flush_pointer(&mut pointer_pending, abs_x, abs_y);
                            flush_wheel(&mut wheel_pending);
                            let display = service_endpoint("hk-server-display");
                            if display != 0 {
                                let _ = send(
                                    display,
                                    INPUT_BUTTON,
                                    if event.value != 0 { 1 } else { 0 },
                                );
                            }
                            continue;
                        }
                        if event.code == KEY_LEFTSHIFT || event.code == KEY_RIGHTSHIFT {
                            shift = event.value != 0;
                            continue;
                        }
                        if event.code == KEY_LEFTCTRL || event.code == KEY_RIGHTCTRL {
                            ctrl = event.value != 0;
                            continue;
                        }
                        if event.code == KEY_LEFTALT || event.code == KEY_RIGHTALT {
                            alt = event.value != 0;
                            continue;
                        }
                        if event.value == 0 {
                            continue;
                        }
                        let Some(keys) = key_bytes(event.code, shift, ctrl, alt) else {
                            continue;
                        };
                        flush_pointer(&mut pointer_pending, abs_x, abs_y);
                        flush_wheel(&mut wheel_pending);
                        let display = service_endpoint("hk-server-display");
                        if display != 0 {
                            for &byte in &keys.bytes[..keys.len] {
                                let _ = send(display, INPUT_KEY, u64::from(byte));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        flush_pointer(&mut pointer_pending, abs_x, abs_y);
        flush_wheel(&mut wheel_pending);
        let _ = wait_notify(notification, 1);
    }
}

/// 调试：把指针在两个固定位置之间来回移动（用于抓帧检查光标残影）。
#[cfg(target_os = "none")]
#[allow(dead_code)]
fn replay_drag() -> ! {
    const POSITIONS: [(u16, u16); 2] = [(6000, 30_000), (26_000, 30_000)];
    loop {
        let display = service_endpoint("hk-server-display");
        if display != 0 {
            let mut index = 0_usize;
            loop {
                let (x, y) = POSITIONS[index % POSITIONS.len()];
                index = index.wrapping_add(1);
                let _ = send(display, INPUT_POINTER, pack_pointer(x, y));
                for _ in 0..5_000_000 {
                    core::hint::spin_loop();
                }
            }
        }
        for _ in 0..50_000 {
            core::hint::spin_loop();
        }
    }
}

/// 把本批最后一次指针位置发给显示服务；没有移动时不发消息。
#[cfg(target_os = "none")]
fn flush_pointer(pending: &mut bool, x: u16, y: u16) {
    if !*pending {
        return;
    }
    *pending = false;
    let display = service_endpoint("hk-server-display");
    if display != 0 {
        let _ = send(display, INPUT_POINTER, pack_pointer(x, y));
    }
}

/// 把本批累积的滚轮增量发给显示服务。
#[cfg(target_os = "none")]
fn flush_wheel(pending: &mut i32) {
    let delta = core::mem::take(pending);
    if delta == 0 {
        return;
    }
    let display = service_endpoint("hk-server-display");
    if display != 0 {
        let _ = send(display, INPUT_WHEEL, pack_wheel(delta));
    }
}

hk_user_api::server_boilerplate!();
