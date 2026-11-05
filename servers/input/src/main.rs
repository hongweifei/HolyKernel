//! input：用户态 virtio-input 驱动。
//!
//! 向 Device Manager 逐个申请输入设备授权（键盘、平板等），把按键事件解码为
//! 字节、把绝对坐标转发给显示服务；显示服务按焦点路由到窗口客户端。

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
use hk_driver_virtio::{Device, Input, probe};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_INPUT, DEVICE_GRANT, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::input::{INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, pack_pointer};
#[cfg(target_os = "none")]
use hk_server_api::{
    device_info, exit, irq_bind, recv_handle, self_notification, send, service_endpoint,
    wait_notify, write, write_u64,
};

#[cfg(target_os = "none")]
mod keymap;

#[cfg(target_os = "none")]
use keymap::{
    ABS_MAX, ABS_X, ABS_Y, BTN_LEFT, BTN_TOUCH, EV_ABS, EV_KEY, KEY_LEFTSHIFT, KEY_RIGHTSHIFT,
    key_byte,
};

/// 堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

/// 最多接管的输入设备数。
#[cfg(target_os = "none")]
const MAX_INPUTS: usize = 4;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

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

        let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
        let length = device_info(handle, &mut bytes) as usize;
        if length == u64::MAX as usize {
            continue;
        }
        let Some(descriptor) = DeviceGrant::decode(&bytes[..length]) else {
            continue;
        };
        match probe(handle, descriptor) {
            Ok(Device::Input(input)) => {
                if irq_bind(handle, notification, 1) != 0 {
                    write(b"hk-server-input: irq bind failed\n");
                    continue;
                }
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

    if inputs.is_empty() {
        write(b"hk-server-input: no input device\n");
        exit(0)
    }
    write(b"hk-server-input: ready\n");
    write(b"hk-server-input: devices ");
    write_u64(inputs.len() as u64);
    write(b"\n");

    let mut shift = false;
    let mut abs_x = 0_u16;
    let mut abs_y = 0_u16;
    loop {
        for input in &mut inputs {
            input.ack_interrupt();
            while let Some(event) = input.pop_event() {
                match event.event_type {
                    EV_ABS => match event.code {
                        ABS_X => {
                            abs_x = event.value.min(ABS_MAX) as u16;
                            forward_pointer(abs_x, abs_y);
                        }
                        ABS_Y => {
                            abs_y = event.value.min(ABS_MAX) as u16;
                            forward_pointer(abs_x, abs_y);
                        }
                        _ => {}
                    },
                    EV_KEY => {
                        if event.code == BTN_LEFT || event.code == BTN_TOUCH {
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
                        if event.value == 0 {
                            continue;
                        }
                        let Some(byte) = key_byte(event.code, shift) else {
                            continue;
                        };
                        let display = service_endpoint("hk-server-display");
                        if display != 0 {
                            let _ = send(display, INPUT_KEY, u64::from(byte));
                        }
                    }
                    _ => {}
                }
            }
        }
        let _ = wait_notify(notification, 1);
    }
}

/// 转发绝对指针位置。
#[cfg(target_os = "none")]
fn forward_pointer(x: u16, y: u16) {
    let display = service_endpoint("hk-server-display");
    if display != 0 {
        let _ = send(display, INPUT_POINTER, pack_pointer(x, y));
    }
}

hk_server_api::server_boilerplate!();
