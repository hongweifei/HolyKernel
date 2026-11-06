//! virtio-console 字节流处理。

use hk_driver_virtio::Console;
use hk_driver_virtio::protocol::vconsole::{
    VCONSOLE_ACK, VCONSOLE_DATA, VCONSOLE_MAX, VCONSOLE_PING, VCONSOLE_PONG, VCONSOLE_READ,
    VCONSOLE_SHUTDOWN, VCONSOLE_WRITE,
};
use hk_server_api::{exit, send, send_data, yield_now};

/// 自检写入的测试行。
const TEST_MESSAGE: &[u8] = b"vcon hello\n";
/// 自检等待的宿主回复。
const ECHO_TOKEN: &[u8] = b"vcon-ack";
/// 自检轮次上限。
const SELF_TEST_ATTEMPTS: usize = 20_000;

/// 启动自检：写入一行并等待宿主回显。
pub fn self_test(console: &Console) -> bool {
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

/// 处理控制台协议消息。
pub fn handle(console: &Console, label: u64, word: u64, sender: u64, data: &[u8]) {
    match label {
        VCONSOLE_WRITE => {
            let data = &data[..data.len().min(VCONSOLE_MAX)];
            let written = match console.send(data) {
                Ok(()) => data.len(),
                Err(_) => 0,
            };
            let _ = send(sender, VCONSOLE_ACK, written as u64);
        }
        VCONSOLE_READ => {
            let _ = console.ack_interrupt();
            let mut output = [0_u8; VCONSOLE_MAX];
            let mut length = 0;
            while length < output.len() {
                let Some(byte) = console.receive() else {
                    break;
                };
                output[length] = byte;
                length += 1;
            }
            let _ = send_data(sender, VCONSOLE_DATA, &output[..length]);
        }
        VCONSOLE_PING => {
            let _ = send(sender, VCONSOLE_PONG, 0);
        }
        VCONSOLE_SHUTDOWN => exit(0),
        _ => {
            let _ = word;
        }
    }
}
