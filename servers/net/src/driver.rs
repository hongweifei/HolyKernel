//! 驱动交互：轮询、共享帧附加与驱动重启。

use core::net::Ipv4Addr;

use hk_net::Stack;
use hk_server_api::net::{
    NET_FRAME_ATTACH, NET_FRAME_MAX, NET_FRAME_ROLE_RX, NET_FRAME_ROLE_TX, NET_INFO_BYTES,
    NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_TX, NET_POLL, NET_POLL_DONE, NET_RX,
    NET_RX_FRAME, NET_SHARED_FRAME, NET_SHARED_RX, NET_SHARED_TX, NET_SHUTDOWN, NET_TX_DONE,
    net_frame_index, net_frame_role, net_rx_index, net_rx_length, net_rx_offset, net_tx_index,
};
use hk_server_api::{
    capability_phys, close_handle, map_attach, notify, recv_data_handle, recv_data_try,
    region_unmap, send, service_endpoint, write, yield_now,
};

use crate::protocol::flush_tx;
use crate::{POLL_WAIT_MS, Stats, WAIT_MS, now_ms};

#[cfg(target_os = "none")]
pub(crate) fn poll_once(
    driver: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) {
    // 先入队再通知：发送为异步，消息已在端点队列中，唤醒不会丢失。
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    // 有界等待完成消息：即使偶发丢失 NET_POLL_DONE，也不会永久阻塞在接收上。
    let deadline = now_ms().saturating_add(POLL_WAIT_MS);
    loop {
        let (label, word, _sender, length) = recv_data_try(endpoint, frame);
        match label {
            NET_RX => {
                let length = length.min(frame.len());
                let _ = stack.push_frame(&frame[..length]);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
            }
            NET_RX_FRAME => {
                let length = net_rx_length(word);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
                stack.push_shared_rx(net_rx_index(word), net_rx_offset(word), length);
            }
            NET_TX_DONE => stack.shared_tx_done(net_tx_index(word)),
            NET_POLL_DONE => break,
            0 => {
                if now_ms() >= deadline {
                    break;
                }
                let _ = yield_now();
            }
            _ => {}
        }
    }
    stack.poll(now_ms());
    flush_tx(driver, stack, stats);
}

/// 接收驱动移交的共享帧并映射到本任务地址空间。
#[cfg(target_os = "none")]
pub(crate) fn attach_shared(driver: u64, endpoint: u64, stack: &mut Stack) -> bool {
    let mut buffer = [0_u8; 8];
    let mut rx = [0_u64; NET_SHARED_RX];
    let mut tx = [0_u64; NET_SHARED_TX];
    let mut rx_seen = 0_usize;
    let mut tx_seen = 0_usize;
    while rx_seen < NET_SHARED_RX || tx_seen < NET_SHARED_TX {
        let (label, word, sender, _length, handle) =
            recv_data_handle(endpoint, &mut buffer, driver);
        if label != NET_FRAME_ATTACH || sender != driver || handle == 0 {
            return false;
        }
        let role = net_frame_role(word);
        let index = net_frame_index(word);
        let (virts, count) = match role {
            NET_FRAME_ROLE_RX => (&mut rx, NET_SHARED_RX),
            NET_FRAME_ROLE_TX => (&mut tx, NET_SHARED_TX),
            _ => {
                let _ = close_handle(handle);
                return false;
            }
        };
        if index >= count {
            let _ = close_handle(handle);
            return false;
        }
        if virts[index] != 0 {
            let _ = close_handle(handle);
            return false;
        }
        let Some((region, virt)) = map_attach(handle) else {
            let _ = close_handle(handle);
            return false;
        };
        if capability_phys(handle).is_none_or(|(_, length)| length != NET_SHARED_FRAME as u64) {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
            return false;
        }
        virts[index] = virt;
        if role == NET_FRAME_ROLE_RX {
            rx_seen += 1;
        } else {
            tx_seen += 1;
        }
    }
    stack.enable_shared(rx.to_vec(), tx.to_vec(), NET_SHARED_FRAME);
    write("hk-server-net: shared frames ready\n".as_bytes());
    true
}

/// 请求驱动退出，等待 Root Server 重启后重新握手并验证链路。
#[cfg(target_os = "none")]
pub(crate) fn restart_network(
    old: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
    gateway: Ipv4Addr,
) -> u64 {
    write("hk-server-net: driver restart\n".as_bytes());
    let _ = send(old, NET_SHUTDOWN, 0);
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut driver = old;
    while now_ms() < deadline {
        let _ = recv_data_try(endpoint, frame);
        let candidate = service_endpoint("hk-server-virtio-net");
        if candidate != 0 && candidate != old {
            driver = candidate;
            break;
        }
        let _ = yield_now();
    }
    if driver == old {
        write("hk-server-net: driver restart failed\n".as_bytes());
        return old;
    }

    let mut info = [0_u8; NET_INFO_BYTES];
    let _ = send(driver, NET_INFO_REQUEST, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut rehandshaked = false;
    while now_ms() < deadline {
        let (label, _word, sender, length) = recv_data_try(endpoint, &mut info);
        if label == NET_INFO_RESPONSE && sender == driver && length >= NET_INFO_BYTES {
            rehandshaked = true;
            break;
        }
        let _ = yield_now();
    }
    if !rehandshaked {
        write("hk-server-net: driver rehandshake failed\n".as_bytes());
        return driver;
    }

    let Ok(icmp) = stack.open_icmp(0x1235) else {
        write("hk-server-net: restart icmp failed\n".as_bytes());
        return driver;
    };
    let _ = stack.send_ping(icmp, gateway, 0x1235, 1, b"holy");
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut reply = [0_u8; 64];
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if let Ok(Some((length, _))) = stack.recv_ping(icmp, &mut reply)
            && length >= 8
        {
            stack.close_socket(icmp);
            write("hk-server-net: restart ok\n".as_bytes());
            return driver;
        }
    }
    stack.close_socket(icmp);
    write("hk-server-net: restart ping failed\n".as_bytes());
    driver
}

#[cfg(target_os = "none")]
pub(crate) fn wait_driver() -> u64 {
    hk_server_api::runtime::wait_service("hk-server-virtio-net").unwrap_or(0)
}
