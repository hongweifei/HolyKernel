//! 协议标签区间。
//!
//! 线协议的 u64 标签按服务分段：每个服务占一个 `0x100` 区间，标签在区间内
//! 分配，新版协议先占一段再追加。各协议的标签由这里的基址派生，新增标签先
//! 登记到 [`LABEL_GROUPS`]，由测试检查分段与唯一性。

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
/// virtio 驱动服务（熵源与控制台）协议基址。
pub const VIRTIO: u64 = 0x1000;
/// Memory Server 协议基址。
pub const MEMORY: u64 = 0x1100;
/// 内核与 Provider 之间的缺页协议基址。
pub const PROVIDER: u64 = 0x1200;
/// Tempo 调度策略协议基址。
pub const TEMPO: u64 = 0x1300;

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
    ("virtio", VIRTIO),
    ("memory", MEMORY),
    ("provider", PROVIDER),
    ("tempo", TEMPO),
];

/// 已登记的协议标签；每组的第一项是基址。
pub const LABEL_GROUPS: &[(&str, u64, &[u64])] = &[
    (
        "vfs",
        VFS,
        &[
            crate::vfs::VFS_REQUEST,
            crate::vfs::VFS_RESPONSE,
            crate::vfs::VFS_ATTACH,
            crate::vfs::VFS_DETACH,
        ],
    ),
    (
        "device-block",
        DEVICE_BLOCK,
        &[
            crate::block::BLOCK_REQUEST,
            crate::block::BLOCK_RESPONSE,
            crate::block::BLOCK_GRANT,
            crate::block::BLOCK_NO_DEVICE,
            crate::block::DEVICE_REQUEST,
            crate::block::BLOCK_SHUTDOWN,
            crate::block::BLOCK_ATTACH,
            crate::block::BLOCK_DETACH,
        ],
    ),
    (
        "net",
        NET,
        &[
            crate::net::NET_INFO_REQUEST,
            crate::net::NET_INFO_RESPONSE,
            crate::net::NET_SHUTDOWN,
            crate::net::NET_TX,
            crate::net::NET_RX,
            crate::net::NET_POLL,
            crate::net::NET_POLL_DONE,
            crate::net::NET_STATUS_REQUEST,
            crate::net::NET_STATUS_RESPONSE,
            crate::net::NET_ARP_REQUEST,
            crate::net::NET_ARP_RESPONSE,
            crate::net::NET_CONFIG_REQUEST,
            crate::net::NET_CONFIG_RESPONSE,
            crate::net::NET_MAPPING_ATTACH,
            crate::net::NET_RX_FRAME,
            crate::net::NET_RX_REFILL,
            crate::net::NET_TX_FRAME,
            crate::net::NET_TX_DONE,
        ],
    ),
    (
        "socket",
        SOCKET,
        &[
            crate::socket::SOCK_OPEN,
            crate::socket::SOCK_CONNECT,
            crate::socket::SOCK_SEND,
            crate::socket::SOCK_RECV,
            crate::socket::SOCK_CLOSE,
            crate::socket::SOCK_UDP_SEND,
            crate::socket::SOCK_RESOLVE,
            crate::socket::SOCK_RESPONSE,
            crate::socket::SOCK_UDP_RECV,
        ],
    ),
    (
        "console",
        CONSOLE,
        &[
            crate::console::CONSOLE_WRITE,
            crate::console::CONSOLE_READ_REQUEST,
            crate::console::CONSOLE_READ_LINE,
            crate::console::CONSOLE_SHUTDOWN,
            crate::console::CONSOLE_WRITE_ACK,
            crate::console::CONSOLE_CLEAR,
            crate::console::CONSOLE_PING,
            crate::console::CONSOLE_PONG,
        ],
    ),
    (
        "display",
        DISPLAY,
        &[
            crate::display::DISPLAY_SHUTDOWN,
            crate::display::DISPLAY_KEY,
            crate::display::DISPLAY_SET_TITLE,
            crate::display::DISPLAY_CLICK,
            crate::display::DISPLAY_CLOSE,
            crate::display::DISPLAY_CLICK_AT,
            crate::display::DISPLAY_RESTART,
            crate::display::DISPLAY_WHEEL,
            crate::display::DISPLAY_RESIZE,
            crate::display::DISPLAY_POINTER,
            crate::display::DISPLAY_POINTER_LEAVE,
            crate::display::SURFACE_SET_CURSOR,
            crate::display::SURFACE_CREATE,
            crate::display::SURFACE_CREATED,
            crate::display::SURFACE_DAMAGE,
            crate::display::SURFACE_SET_INPUT_REGION,
            crate::display::SURFACE_COMMIT,
            crate::display::SURFACE_FRAME_DONE,
            crate::display::SURFACE_DESTROY,
            crate::display::WINDOW_CREATE,
            crate::display::WINDOW_CREATED,
            crate::display::WINDOW_RESIZE,
            crate::display::SEAT_FOCUS,
            crate::display::SEAT_SET_CLIPBOARD,
            crate::display::SEAT_GET_CLIPBOARD,
            crate::display::SEAT_CLIPBOARD,
        ],
    ),
    (
        "input",
        INPUT,
        &[
            crate::input::INPUT_KEY,
            crate::input::INPUT_POINTER,
            crate::input::INPUT_BUTTON,
            crate::input::INPUT_WHEEL,
        ],
    ),
    (
        "checkpoint",
        CHECKPOINT,
        &[
            crate::checkpoint::CHECKPOINT_SAVE,
            crate::checkpoint::CHECKPOINT_RESTORE,
            crate::checkpoint::CHECKPOINT_LIST,
            crate::checkpoint::CHECKPOINT_RESULT,
        ],
    ),
    ("kernel-event", KERNEL_EVENT, &[hk_syscall::FAULT_EVENT]),
    (
        "echo",
        ECHO,
        &[
            crate::echo::WORK_REQUEST,
            crate::echo::WORK_RESPONSE,
            crate::echo::WORK_STATS,
            crate::echo::WORK_STATS_RESPONSE,
            crate::echo::WORK_DONE,
        ],
    ),
    (
        "holy",
        HOLY,
        &[crate::holy::HOLY_REQUEST, crate::holy::HOLY_RESPONSE],
    ),
    (
        "memory",
        MEMORY,
        &[
            hk_server_api::memory::MEMORY_ALLOC,
            hk_server_api::memory::MEMORY_ALLOCATED,
            hk_server_api::memory::MEMORY_FRAME,
            hk_server_api::memory::MEMORY_FRAME_ALLOCATED,
        ],
    ),
    (
        "provider",
        PROVIDER,
        &[hk_syscall::PROVIDER_FAULT, hk_syscall::PROVIDER_PROVIDE],
    ),
    (
        "tempo",
        TEMPO,
        &[
            crate::tempo::TEMPO_REQUEST,
            crate::tempo::TEMPO_RESPONSE,
            crate::tempo::TEMPO_REJECTED,
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

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
        for (name, base, labels) in LABEL_GROUPS {
            for label in *labels {
                assert_eq!(base_of(*label), *base, "{name} 的标签越出分段：{label:#x}");
            }
        }
    }

    #[test]
    fn labels_are_unique_across_protocols() {
        for (name, _, labels) in LABEL_GROUPS {
            for (index, label) in labels.iter().enumerate() {
                for other in &labels[index + 1..] {
                    assert_ne!(label, other, "{name} 段内标签重复：{label:#x}");
                }
            }
            for (other, _, other_labels) in LABEL_GROUPS {
                if name == other {
                    continue;
                }
                for label in *labels {
                    for other_label in *other_labels {
                        assert_ne!(label, other_label, "{name} 与 {other} 共用标签：{label:#x}");
                    }
                }
            }
        }
    }

    #[test]
    fn kernel_event_and_services_do_not_share_labels() {
        assert_eq!(base_of(hk_syscall::FAULT_EVENT), KERNEL_EVENT);
        assert_eq!(base_of(hk_syscall::PROVIDER_FAULT), PROVIDER);
        assert_eq!(base_of(hk_syscall::PROVIDER_PROVIDE), PROVIDER);
    }
}
