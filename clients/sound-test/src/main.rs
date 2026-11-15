//! sound-test：virtio 音频服务自检客户端。
//!
//! 查询音频能力，再请求一段 880 Hz 正弦音；音高由冒烟在 WAV 后端上校验。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_proto::sound::Client;
#[cfg(target_os = "none")]
use hk_server_api::{boot_cmdline, exit, format::Line, ticks, write, yield_now};

/// 自检音频率（Hz）。
#[cfg(target_os = "none")]
const TEST_TONE_HZ: u32 = 880;
/// 自检音时长（毫秒）：足够长，用来验证播放期间服务仍可响应请求。
#[cfg(target_os = "none")]
const TEST_TONE_MILLIS: u32 = 1_000;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut buffer = [0_u8; 128];
    if !boot_cmdline(&mut buffer).is_some_and(|line| line.contains("sound-test")) {
        exit(0)
    }
    let Some(client) = Client::discover(endpoint) else {
        write(b"hk-server-sound-test: service not found\n");
        exit(1)
    };
    let Some((streams, rate, channels, jacks)) = client.info() else {
        write(b"hk-server-sound-test: info failed\n");
        exit(2)
    };
    Line::new()
        .text(b"hk-server-sound-test: info streams ")
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
    if !client.play(TEST_TONE_HZ, TEST_TONE_MILLIS) {
        write(b"hk-server-sound-test: tone failed\n");
        exit(3)
    }
    // 受理应当是异步的：1 秒音频不能让请求本身阻塞。
    let accepted = ticks();
    if accepted.saturating_sub(started) > 100 {
        Line::new()
            .text(b"hk-server-sound-test: play blocked service ")
            .number(accepted.saturating_sub(started))
            .text(b" ms\n")
            .finish();
        exit(4)
    }
    // 用状态轮询驱动播放推进；服务每次处理消息时补交音频周期。
    let mut polls = 0_u64;
    loop {
        match client.status() {
            1 => polls = polls.saturating_add(1),
            0 => break,
            _ => {
                write(b"hk-server-sound-test: playback failed\n");
                exit(5)
            }
        }
        if polls > 200_000 {
            write(b"hk-server-sound-test: playback timeout\n");
            exit(6)
        }
        let _ = yield_now();
    }
    let finished = ticks();
    Line::new()
        .text(b"hk-server-sound-test: tone ")
        .number(u64::from(TEST_TONE_HZ))
        .text(b" ok\n")
        .finish();
    Line::new()
        .text(b"hk-server-sound-test: async accept ")
        .number(accepted.saturating_sub(started))
        .text(b" ms polls ")
        .number(polls)
        .text(b" total ")
        .number(finished.saturating_sub(started))
        .text(b" ms\n")
        .finish();
    exit(0)
}

hk_server_api::server_boilerplate!();
