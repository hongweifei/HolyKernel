//! 共享帧路径：分配、DMA 登记、附加与收发搬运。

use hk_driver_virtio::{Network, register_frame};
use hk_server_api::net::{
    NET_FRAME_ATTACH, NET_FRAME_ROLE_RX, NET_FRAME_ROLE_TX, NET_RX, NET_RX_FRAME, NET_SHARED_FRAME,
    NET_SHARED_RX, NET_SHARED_TX, net_frame_word, net_rx_word,
};
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, dma_map_frame, frame_alloc, frame_info,
    handle_duplicate, send, send_data, send_handle, write,
};

/// 驱动侧共享接收帧虚拟基址。
#[cfg(target_os = "none")]
const SHARED_RX_BASE: u64 = 0x7000_0000;
/// 驱动侧共享发送帧虚拟基址。
#[cfg(target_os = "none")]
const SHARED_TX_BASE: u64 = 0x7010_0000;

/// 解析是否启用共享帧路径。
#[cfg(target_os = "none")]
pub(crate) fn zerocopy_enabled() -> bool {
    let mut buffer = [0_u8; 128];
    hk_server_api::boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.zerocopy"))
}

/// 分配共享帧、登记 DMA 并把副本句柄交给 Network Server；成功后提交接收缓冲。
#[cfg(target_os = "none")]
pub(crate) fn attach_shared(network: &mut Network, peer: u64, grant: u64) -> bool {
    if !attach_role(
        peer,
        grant,
        NET_FRAME_ROLE_RX,
        SHARED_RX_BASE,
        NET_SHARED_RX,
    ) || !attach_role(
        peer,
        grant,
        NET_FRAME_ROLE_TX,
        SHARED_TX_BASE,
        NET_SHARED_TX,
    ) {
        write("hk-server-virtio-net: shared frame attach failed\n".as_bytes());
        return false;
    }
    network.set_shared_layout(SHARED_RX_BASE, SHARED_TX_BASE, NET_SHARED_FRAME);
    for index in 0..NET_SHARED_RX {
        if !network.submit_shared_rx(index) {
            write("hk-server-virtio-net: shared rx submit failed\n".as_bytes());
            return false;
        }
    }
    write("hk-server-virtio-net: shared frames ready\n".as_bytes());
    true
}

#[cfg(target_os = "none")]
fn attach_role(peer: u64, grant: u64, role: u64, base: u64, count: usize) -> bool {
    for index in 0..count {
        let virt = base + (index * NET_SHARED_FRAME) as u64;
        let handle = frame_alloc(NET_SHARED_FRAME, virt);
        if handle == 0 || handle == u64::MAX {
            return false;
        }
        if let Some((phys, _)) = frame_info(handle) {
            register_frame(virt, phys, NET_SHARED_FRAME);
        }
        let _ = dma_map_frame(grant, handle, NET_SHARED_FRAME);
        let duplicate = handle_duplicate(handle, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        if duplicate == u64::MAX {
            return false;
        }
        if send_handle(
            peer,
            NET_FRAME_ATTACH,
            net_frame_word(role, index),
            duplicate,
        ) == u64::MAX
        {
            return false;
        }
    }
    true
}

#[cfg(target_os = "none")]
pub(crate) fn drain_rx(network: &hk_driver_virtio::Network, peer: u64) {
    if peer == 0 {
        return;
    }
    while let Some(frame) = network.receive() {
        let _ = send_data(peer, NET_RX, &frame);
    }
}

#[cfg(target_os = "none")]
pub(crate) fn drain_shared_rx(network: &hk_driver_virtio::Network, peer: u64) {
    if peer == 0 {
        return;
    }
    while let Some((index, offset, length)) = network.poll_shared_rx() {
        let _ = send(peer, NET_RX_FRAME, net_rx_word(index, offset, length));
    }
}
