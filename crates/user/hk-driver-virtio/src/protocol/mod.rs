//! virtio 设备服务的线协议与客户端。
//!
//! 这些是用户态驱动层协议，留在驱动 crate 里；标签从 `hk_proto::labels` 的
//! virtio 段派生，由本 crate 的单测锁定。

pub mod entropy;
pub mod vconsole;

#[cfg(test)]
mod tests {
    use hk_proto::labels::{VIRTIO, base_of};

    use super::{entropy, vconsole};

    #[test]
    fn protocol_labels_stay_in_the_virtio_block() {
        let labels = [
            entropy::ENTROPY_REQUEST,
            entropy::ENTROPY_RESPONSE,
            entropy::ENTROPY_PING,
            entropy::ENTROPY_PONG,
            entropy::ENTROPY_SHUTDOWN,
            vconsole::VCONSOLE_WRITE,
            vconsole::VCONSOLE_READ,
            vconsole::VCONSOLE_DATA,
            vconsole::VCONSOLE_ACK,
            vconsole::VCONSOLE_PING,
            vconsole::VCONSOLE_PONG,
            vconsole::VCONSOLE_SHUTDOWN,
        ];
        for label in labels {
            assert_eq!(base_of(label), VIRTIO, "标签越出 virtio 段：{label:#x}");
        }
    }
}
