//! 原生 Socket 自检：作为普通用户态客户端使用 `hk-server-api` 的 Socket 协议。
//!
//! 仅在 `net.socket-test` 启动参数存在时运行；内核与 Shell 不感知该协议。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::socket::{Client, SocketKind};
#[cfg(target_os = "none")]
use hk_server_api::{boot_cmdline, exit, tsc, tsc_frequency, write, yield_now};

#[cfg(target_os = "none")]
const GATEWAY: [u8; 4] = [10, 0, 2, 2];
#[cfg(target_os = "none")]
const TCP_PORT: u16 = 45678;
#[cfg(target_os = "none")]
const UDP_PORT: u16 = 45679;
#[cfg(target_os = "none")]
const WAIT_MS: u64 = 8_000;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut buffer = [0_u8; 128];
    if !boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.socket-test")) {
        exit(0)
    }
    let Some(client) = Client::discover(endpoint) else {
        write("hk-server-netsock: network server not found\n".as_bytes());
        exit(1)
    };
    if !tcp_test(&client) {
        write("hk-server-netsock: tcp failed\n".as_bytes());
        exit(2)
    }
    write("hk-server-netsock: tcp ok\n".as_bytes());
    if !udp_test(&client) {
        write("hk-server-netsock: udp failed\n".as_bytes());
        exit(3)
    }
    write("hk-server-netsock: udp ok\n".as_bytes());
    if !dns_test(&client) {
        write("hk-server-netsock: dns failed\n".as_bytes());
        exit(4)
    }
    write("hk-server-netsock: dns ok\n".as_bytes());
    exit(0)
}

#[cfg(target_os = "none")]
fn tcp_test(client: &Client) -> bool {
    let Some(handle) = client.open(SocketKind::Tcp) else {
        return false;
    };
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut connected = false;
    while now_ms() < deadline {
        if client.connect(handle, GATEWAY, TCP_PORT) {
            connected = true;
            break;
        }
        let _ = yield_now();
    }
    if !connected {
        client.close(handle);
        return false;
    }

    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut sent = false;
    while now_ms() < deadline {
        if client.send(handle, b"holy") >= 4 {
            sent = true;
            break;
        }
        let _ = yield_now();
    }
    let mut ok = false;
    if sent {
        let mut reply = [0_u8; 8];
        let deadline = now_ms().saturating_add(WAIT_MS);
        while now_ms() < deadline {
            if client.recv(handle, &mut reply) >= 4 {
                ok = true;
                break;
            }
            let _ = yield_now();
        }
    }
    client.close(handle);
    ok
}

#[cfg(target_os = "none")]
fn udp_test(client: &Client) -> bool {
    let Some(handle) = client.open(SocketKind::Udp) else {
        return false;
    };
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut reply = [0_u8; 8];
    let mut ok = false;
    let mut last_send = 0;
    while now_ms() < deadline {
        let now = now_ms();
        if now.saturating_sub(last_send) >= 200 {
            let _ = client.udp_send(handle, GATEWAY, UDP_PORT, b"holy");
            last_send = now;
        }
        if client.udp_recv(handle, &mut reply) >= 4 {
            ok = true;
            break;
        }
        let _ = yield_now();
    }
    client.close(handle);
    ok
}

#[cfg(target_os = "none")]
fn dns_test(client: &Client) -> bool {
    let mut addresses = [0_u8; 16];
    let count = client.resolve("localhost", &mut addresses);
    count >= 1 && addresses[..4] == [127, 0, 0, 1]
}

#[cfg(target_os = "none")]
fn now_ms() -> u64 {
    let frequency = tsc_frequency();
    if frequency == 0 {
        return hk_server_api::ticks().saturating_mul(10);
    }
    tsc().saturating_mul(1_000) / frequency
}

hk_server_api::server_boilerplate!();
