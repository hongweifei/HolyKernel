//! input：用户态 virtio-input 驱动。
//!
//! 向 Device Manager 申请输入设备授权，把按键事件解码为字节后发给显示服务；
//! 显示服务按焦点路由到窗口客户端。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::string::ToString;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, probe};
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
    wait_notify, write,
};

/// Linux evdev 事件类型：按键。
#[cfg(target_os = "none")]
const EV_KEY: u16 = 1;
/// Linux evdev 事件类型：绝对坐标。
#[cfg(target_os = "none")]
const EV_ABS: u16 = 3;
/// 绝对 X 轴。
#[cfg(target_os = "none")]
const ABS_X: u16 = 0;
/// 绝对 Y 轴。
#[cfg(target_os = "none")]
const ABS_Y: u16 = 1;
/// 左 Shift。
#[cfg(target_os = "none")]
const KEY_LEFTSHIFT: u16 = 42;
/// 右 Shift。
#[cfg(target_os = "none")]
const KEY_RIGHTSHIFT: u16 = 54;
/// 鼠标左键。
#[cfg(target_os = "none")]
const BTN_LEFT: u16 = 0x110;
/// 触摸/笔接触按钮。
#[cfg(target_os = "none")]
const BTN_TOUCH: u16 = 0x14A;
/// 绝对坐标最大值。
#[cfg(target_os = "none")]
const ABS_MAX: u32 = 0x7FFF;

/// 堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;

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
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_INPUT);
    let (label, _word, _sender, handle) = recv_handle(endpoint);
    if label != DEVICE_GRANT || handle == 0 {
        write(b"hk-server-input: no input device\n");
        exit(0)
    }

    let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
    let length = device_info(handle, &mut bytes) as usize;
    if length == u64::MAX as usize {
        write(b"hk-server-input: bad grant\n");
        exit(2)
    }
    let Some(descriptor) = DeviceGrant::decode(&bytes[..length]) else {
        write(b"hk-server-input: bad descriptor\n");
        exit(2)
    };
    let mut input = match probe(handle, descriptor) {
        Ok(Device::Input(input)) => input,
        Ok(_) => {
            write(b"hk-server-input: not an input device\n");
            exit(3)
        }
        Err(error) => {
            write(b"hk-server-input: ");
            write(error.to_string().as_bytes());
            write(b"\n");
            exit(4)
        }
    };

    let notification = self_notification();
    if irq_bind(handle, notification, 1) != 0 {
        write(b"hk-server-input: irq bind failed\n");
        exit(5)
    }
    write(b"hk-server-input: ready\n");

    let mut shift = false;
    let mut abs_x = 0_u16;
    let mut abs_y = 0_u16;
    loop {
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
                            let _ =
                                send(display, INPUT_BUTTON, if event.value != 0 { 1 } else { 0 });
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

/// 把 evdev 键码映射为字节。
#[cfg(target_os = "none")]
fn key_byte(code: u16, shift: bool) -> Option<u8> {
    let plain = match code {
        2..=11 => b'1' + (code as u8 - 2),
        12 => b'-',
        13 => b'=',
        14 => 0x08,
        15 => b'\t',
        16..=25 => b"qwertyuiop"[usize::from(code - 16)],
        26 => b'[',
        27 => b']',
        28 => b'\n',
        30..=38 => b"asdfghjkl"[usize::from(code - 30)],
        39 => b';',
        40 => b'\'',
        41 => b'`',
        43 => b'\\',
        44..=50 => b"zxcvbnm"[usize::from(code - 44)],
        51 => b',',
        52 => b'.',
        53 => b'/',
        57 => b' ',
        _ => return None,
    };
    Some(if shift { shifted(plain) } else { plain })
}

/// 应用 Shift 映射。
#[cfg(target_os = "none")]
fn shifted(byte: u8) -> u8 {
    match byte {
        b'a'..=b'z' => byte - 32,
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b';' => b':',
        b'\'' => b'"',
        b'`' => b'~',
        b'\\' => b'|',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        other => other,
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-input 只在 x86_64-unknown-none 目标上构建");
}
