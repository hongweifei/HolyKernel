//! 协议标签区间。
//!
//! 线协议的 u64 标签按服务分段：每个服务占一个 `0x100` 区间，标签在区间内
//! 分配，新版协议先占一段再追加。不同服务之间不共用基址，避免新增协议时撞
//! 号。

/// VFS 协议基址。
pub const VFS: u64 = 0x500;
/// 设备授权与块协议基址。
pub const DEVICE_BLOCK: u64 = 0x600;
/// 网络协议基址。
pub const NET: u64 = 0x700;
/// 原生 Socket 协议基址。
pub const SOCKET: u64 = 0x800;
/// Console 协议基址。
pub const CONSOLE: u64 = 0x900;
/// Display 与表面协议基址。
pub const DISPLAY: u64 = 0xA00;
/// 输入事件基址。
pub const INPUT: u64 = 0xB00;
/// Checkpoint 协议基址。
pub const CHECKPOINT: u64 = 0xC00;
/// 内核投递给订阅者的事件基址。
pub const KERNEL_EVENT: u64 = 0xD00;
/// echo 演示协议基址。
pub const ECHO: u64 = 0xE00;
/// Holy 文档协议基址。
pub const HOLY: u64 = 0xF00;

/// 返回标签所属区间的基址。
pub const fn base_of(label: u64) -> u64 {
    label & !0xFF
}

/// 所有协议基址。
pub const BASES: &[(&str, u64)] = &[
    ("vfs", VFS),
    ("device-block", DEVICE_BLOCK),
    ("net", NET),
    ("socket", SOCKET),
    ("console", CONSOLE),
    ("display", DISPLAY),
    ("input", INPUT),
    ("checkpoint", CHECKPOINT),
    ("kernel-event", KERNEL_EVENT),
    ("echo", ECHO),
    ("holy", HOLY),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block, checkpoint, console, display, echo, holy, input, net, socket, vfs};

    #[test]
    fn protocol_bases_are_distinct() {
        for (index, (name, base)) in BASES.iter().enumerate() {
            assert_eq!(*base & 0xFF, 0, "{name} 基址对齐到 0x100");
            for (other, other_base) in &BASES[index + 1..] {
                assert_ne!(base, other_base, "{name} 与 {other} 共用标签基址");
            }
        }
    }

    #[test]
    fn labels_stay_in_their_protocol_block() {
        let cases: &[(&str, u64, &[u64])] = &[
            (
                "vfs",
                VFS,
                &[
                    vfs::VFS_REQUEST,
                    vfs::VFS_RESPONSE,
                    vfs::VFS_ATTACH,
                    vfs::VFS_DETACH,
                ],
            ),
            (
                "block",
                DEVICE_BLOCK,
                &[
                    block::BLOCK_REQUEST,
                    block::BLOCK_RESPONSE,
                    block::BLOCK_GRANT,
                    block::BLOCK_NO_DEVICE,
                    block::DEVICE_REQUEST,
                    block::BLOCK_SHUTDOWN,
                    block::BLOCK_ATTACH,
                    block::BLOCK_DETACH,
                ],
            ),
            ("net", NET, &[net::NET_INFO_REQUEST, net::NET_TX_DONE]),
            (
                "socket",
                SOCKET,
                &[socket::SOCK_OPEN, socket::SOCK_UDP_RECV],
            ),
            (
                "console",
                CONSOLE,
                &[console::CONSOLE_WRITE, console::CONSOLE_PONG],
            ),
            (
                "display",
                DISPLAY,
                &[
                    display::DISPLAY_SHUTDOWN,
                    display::DISPLAY_RESIZE,
                    display::SURFACE_CREATE,
                    display::WINDOW_RESIZE,
                    display::SEAT_FOCUS,
                ],
            ),
            (
                "input",
                INPUT,
                &[
                    input::INPUT_KEY,
                    input::INPUT_POINTER,
                    input::INPUT_BUTTON,
                    input::INPUT_WHEEL,
                ],
            ),
            (
                "checkpoint",
                CHECKPOINT,
                &[
                    checkpoint::CHECKPOINT_SAVE,
                    checkpoint::CHECKPOINT_RESTORE,
                    checkpoint::CHECKPOINT_LIST,
                    checkpoint::CHECKPOINT_RESULT,
                ],
            ),
            (
                "echo",
                ECHO,
                &[
                    echo::WORK_REQUEST,
                    echo::WORK_RESPONSE,
                    echo::WORK_STATS,
                    echo::WORK_STATS_RESPONSE,
                    echo::WORK_DONE,
                ],
            ),
            ("holy", HOLY, &[holy::HOLY_REQUEST, holy::HOLY_RESPONSE]),
        ];
        for (name, base, labels) in cases {
            for label in *labels {
                assert_eq!(base_of(*label), *base, "{name} 的标签越出分段：{label:#x}");
            }
        }
    }

    #[test]
    fn kernel_event_and_services_do_not_share_labels() {
        assert_eq!(base_of(hk_syscall::FAULT_EVENT), KERNEL_EVENT);
        assert_eq!(base_of(hk_syscall::PROVIDER_FAULT), 0x5052_4F00);
    }
}
