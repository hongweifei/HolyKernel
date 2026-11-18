//! sound-test：virtio 音频服务自检客户端。
//!
//! 查询音频能力，同时点播两声部（660 Hz 与 880 Hz）验证多流并发；音高由冒烟在
//! WAV 后端上按频率能量校验。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_proto::sound::Client;
#[cfg(target_os = "none")]
use hk_user_api::{boot_cmdline, exit, format::Line, ticks, write};

/// 并发点播的两个声部：(频率 Hz, 时长 ms)。
#[cfg(target_os = "none")]
const TONES: [(u32, u32); 2] = [(660, 700), (880, 1_000)];

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut buffer = [0_u8; 128];
    if !boot_cmdline(&mut buffer).is_some_and(|line| line.contains("sound-test")) {
        exit(0)
    }
    let Some(client) = Client::discover(endpoint) else {
        write(b"hk-client-sound-test: service not found\n");
        exit(1)
    };
    let Some((streams, rate, channels, jacks)) = client.info() else {
        write(b"hk-client-sound-test: info failed\n");
        exit(2)
    };
    Line::new()
        .text(b"hk-client-sound-test: info streams ")
        .number(u64::from(streams))
        .text(b" rate ")
        .number(u64::from(rate))
        .text(b" channels ")
        .number(u64::from(channels))
        .text(b" jacks ")
        .number(u64::from(jacks))
        .text(b"\n")
        .finish();
    let started = ticks();
    for (hz, millis) in TONES {
        if !client.play(hz, millis) {
            write(b"hk-client-sound-test: tone failed\n");
            exit(3)
        }
    }
    // 受理应当是异步的：两条点播都立即返回，而不是等前一条播完。
    let accepted = ticks();
    if accepted.saturating_sub(started) > 100 {
        Line::new()
            .text(b"hk-client-sound-test: play blocked service ")
            .number(accepted.saturating_sub(started))
            .text(b" ms\n")
            .finish();
        exit(4)
    }
    // 服务在定时节拍上推进每条流，客户端只等两个完成通知。
    for _ in TONES {
        if !client.wait_done() {
            write(b"hk-client-sound-test: playback failed\n");
            exit(5)
        }
    }
    let finished = ticks();
    Line::new()
        .text(b"hk-client-sound-test: concurrent ")
        .number(u64::from(TONES[0].0))
        .text(b"+")
        .number(u64::from(TONES[TONES.len() - 1].0))
        .text(b" ok\n")
        .finish();
    Line::new()
        .text(b"hk-client-sound-test: async accept ")
        .number(accepted.saturating_sub(started))
        .text(b" ms total ")
        .number(finished.saturating_sub(started))
        .text(b" ms\n")
        .finish();
    exit(0)
}

hk_user_api::server_boilerplate!();
