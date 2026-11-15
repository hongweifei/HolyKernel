//! virtio-console 字节流处理。

use hk_driver_virtio::Console;
use hk_driver_virtio::protocol::vconsole::{
    VCONSOLE_ACK, VCONSOLE_DATA, VCONSOLE_MAX, VCONSOLE_PING, VCONSOLE_PONG, VCONSOLE_PORT_HEADER,
    VCONSOLE_READ, VCONSOLE_SHUTDOWN, VCONSOLE_WRITE,
};
use hk_server_api::{exit, send, send_data, yield_now};

/// 自检写入的测试行。
const TEST_MESSAGE: &[u8] = b"vcon hello\n";
/// 自检等待的宿主回复。
const ECHO_TOKEN: &[u8] = b"vcon-ack";
/// 自检轮次上限。
const SELF_TEST_ATTEMPTS: usize = 20_000;

/// 启动自检：写入一行并等待宿主回显。
pub fn self_test(console: &mut Console) -> bool {
    if console.send(TEST_MESSAGE).is_err() {
        return false;
    }
    let mut seen = [0_u8; 64];
    let mut length = 0;
    for _ in 0..SELF_TEST_ATTEMPTS {
        let _ = console.ack_interrupt();
        while length < seen.len() {
            let Some(byte) = console.receive() else {
                break;
            };
            seen[length] = byte;
            length += 1;
        }
        if seen[..length]
            .windows(ECHO_TOKEN.len())
            .any(|window| window == ECHO_TOKEN)
        {
            return true;
        }
        let _ = yield_now();
    }
    false
}

/// 端口 1 及以上的回显自检：写入一行并等待宿主原样回送。
pub fn self_test_port(console: &mut Console, port: usize) -> bool {
    let mut message = [0_u8; 16];
    let prefix = b"vcon p";
    message[..prefix.len()].copy_from_slice(prefix);
    message[prefix.len()] = b'0' + port as u8;
    message[prefix.len() + 1] = b'\n';
    let length = prefix.len() + 2;
    if console.send_on(port, &message[..length]).is_err() {
        return false;
    }
    let mut seen = [0_u8; 32];
    let mut received = 0;
    for _ in 0..SELF_TEST_ATTEMPTS {
        let _ = console.ack_interrupt();
        while received < seen.len() {
            let Some(byte) = console.receive_on(port) else {
                break;
            };
            seen[received] = byte;
            received += 1;
        }
        if seen[..received]
            .windows(length)
            .any(|window| window == &message[..length])
        {
            return true;
        }
        let _ = yield_now();
    }
    false
}

/// 处理控制台协议消息；写入载荷前 4 字节是端口号。
pub fn handle(console: &mut Console, label: u64, word: u64, sender: u64, data: &[u8]) {
    console.poll_control();
    match label {
        VCONSOLE_WRITE => {
            if data.len() < VCONSOLE_PORT_HEADER {
                let _ = send(sender, VCONSOLE_ACK, 0);
                return;
            }
            let port =
                u32::from_le_bytes(data[..VCONSOLE_PORT_HEADER].try_into().unwrap_or([0; 4]));
            let payload = &data[VCONSOLE_PORT_HEADER..];
            let payload = &payload[..payload.len().min(VCONSOLE_MAX - VCONSOLE_PORT_HEADER)];
            let written = match console.send_on(port as usize, payload) {
                Ok(()) => payload.len(),
                Err(_) => 0,
            };
            let _ = send(
                sender,
                VCONSOLE_ACK,
                (u64::from(port) << 32) | written as u64,
            );
        }
        VCONSOLE_READ => {
            let port = word as u32;
            let _ = console.ack_interrupt();
            let mut output = [0_u8; VCONSOLE_MAX];
            let mut length = 0;
            while length < output.len() {
                let Some(byte) = console.receive_on(port as usize) else {
                    break;
                };
                output[length] = byte;
                length += 1;
            }
            let _ = send_data(sender, VCONSOLE_DATA, &output[..length]);
        }
        VCONSOLE_PING => {
            let _ = send(sender, VCONSOLE_PONG, console.ports() as u64);
        }
        VCONSOLE_SHUTDOWN => exit(0),
        _ => {}
    }
}
