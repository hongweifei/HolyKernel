//! virtio-sound 音频处理。

use hk_driver_virtio::{Sound, TONE_HZ, TONE_MILLIS};
use hk_proto::sound::{
    SOUND_INFO, SOUND_INFO_REPLY, SOUND_PING, SOUND_PONG, SOUND_SHUTDOWN, SOUND_TONE,
    SOUND_TONE_ACK,
};
use hk_server_api::{exit, send, send_data};

/// 启动自检：播放一段 440 Hz / 250 ms 正弦波。
pub fn self_test(sound: &Sound) -> bool {
    sound.tone().is_some()
}

/// 处理音频协议消息。
pub fn handle(sound: &Sound, label: u64, word: u64, sender: u64) {
    match label {
        SOUND_INFO => {
            let Some(streams) = sound.output_streams() else {
                return;
            };
            let mut payload = [0_u8; 16];
            payload[0..4].copy_from_slice(&(streams as u32).to_le_bytes());
            payload[4..8].copy_from_slice(&hk_driver_virtio::TONE_RATE.to_le_bytes());
            payload[8..12].copy_from_slice(&1_u32.to_le_bytes());
            payload[12..16].copy_from_slice(&sound.jacks().to_le_bytes());
            let _ = send_data(sender, SOUND_INFO_REPLY, &payload);
        }
        SOUND_TONE => {
            let hz = u32::try_from(word).unwrap_or(0);
            let hz = if hz == 0 { TONE_HZ } else { hz };
            let ok = sound.tone_at(hz, TONE_MILLIS).is_some();
            let _ = send(sender, SOUND_TONE_ACK, u64::from(ok));
        }
        SOUND_PING => {
            let _ = send(sender, SOUND_PONG, 0);
        }
        SOUND_SHUTDOWN => exit(0),
        _ => {}
    }
}
