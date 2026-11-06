//! virtio-rng 熵源处理。

use hk_driver_virtio::Rng;
use hk_driver_virtio::protocol::entropy::{
    ENTROPY_MAX, ENTROPY_PING, ENTROPY_PONG, ENTROPY_REQUEST, ENTROPY_RESPONSE, ENTROPY_SHUTDOWN,
};
use hk_server_api::{exit, send, send_data};

/// 启动自检：读取一段随机字节。
pub fn self_test(rng: &Rng) -> bool {
    let mut sample = [0_u8; 16];
    matches!(rng.fill(&mut sample), Ok(length) if length == sample.len())
}

/// 处理熵源协议消息。
pub fn handle(rng: &Rng, label: u64, word: u64, sender: u64) {
    match label {
        ENTROPY_REQUEST => {
            let wanted = (word as usize).min(ENTROPY_MAX);
            let mut output = [0_u8; ENTROPY_MAX];
            match rng.fill(&mut output[..wanted]) {
                Ok(length) => {
                    let _ = send_data(sender, ENTROPY_RESPONSE, &output[..length]);
                }
                Err(_) => {
                    let _ = send_data(sender, ENTROPY_RESPONSE, &[]);
                }
            }
        }
        ENTROPY_PING => {
            let _ = send(sender, ENTROPY_PONG, 0);
        }
        ENTROPY_SHUTDOWN => exit(0),
        _ => {}
    }
}
