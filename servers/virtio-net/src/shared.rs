//! 共享帧路径：分配、DMA 登记、附加与收发搬运。

use hk_driver_virtio::{Network, register_mapping};
use hk_proto::net::{
    NET_MAPPING_ATTACH, NET_MAPPING_ROLE_RX, NET_MAPPING_ROLE_TX, NET_RX, NET_RX_FRAME,
    NET_SHARED_LEN, NET_SHARED_RX, NET_SHARED_TX, net_mapping_word, net_rx_word,
};
use hk_server_api::{
    CapabilityKind, MemoryClient, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, capability_phys,
    dma_map_capability, handle_duplicate, send, send_data, send_handle, write,
};

/// 解析是否启用共享帧路径。
#[cfg(target_os = "none")]
pub(crate) fn zerocopy_enabled() -> bool {
    let mut buffer = [0_u8; 128];
    hk_server_api::boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.zerocopy"))
}

/// 分配共享帧、登记 DMA 并把副本句柄交给 Network Server；成功后提交接收缓冲。
#[cfg(target_os = "none")]
pub(crate) fn attach_shared(network: &mut Network, own: u64, peer: u64, grant: u64) -> bool {
    let mut rx = [0_u64; NET_SHARED_RX];
    let mut tx = [0_u64; NET_SHARED_TX];
    if !attach_role(own, peer, grant, NET_MAPPING_ROLE_RX, &mut rx)
        || !attach_role(own, peer, grant, NET_MAPPING_ROLE_TX, &mut tx)
    {
        write("hk-server-virtio-net: shared frame attach failed\n".as_bytes());
        return false;
    }
    network.set_shared_layout(rx, tx, NET_SHARED_LEN);
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
fn attach_role(own: u64, peer: u64, grant: u64, role: u64, virts: &mut [u64]) -> bool {
    for (index, mapping_virt) in virts.iter_mut().enumerate() {
        let Some((handle, _region, virt)) =
            MemoryClient::new(own).map(CapabilityKind::Anonymous, NET_SHARED_LEN)
        else {
            return false;
        };
        *mapping_virt = virt;
        if let Some((phys, _)) = capability_phys(handle) {
            register_mapping(virt, phys, NET_SHARED_LEN);
        }
        let _ = dma_map_capability(grant, handle, NET_SHARED_LEN);
        let duplicate = handle_duplicate(handle, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        if duplicate == u64::MAX {
            return false;
        }
        if send_handle(
            peer,
            NET_MAPPING_ATTACH,
            net_mapping_word(role, index),
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
