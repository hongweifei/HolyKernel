//! virtio-sound 设备封装。
//!
//! 只做播放：取第一个输出流，设置 48 kHz / 单声道 / S16 参数，准备并启动，
//! 送一段正弦波，最后停止并释放。采样参数与格式都先查设备支持位，不支持就
//! 直接失败，不猜设备能力。

use alloc::vec::Vec;

use hk_device::DeviceGrant;
use spin::Mutex;
use virtio_drivers::device::sound::{
    PcmFeatures, PcmFormat, PcmFormats, PcmRate, PcmRates, VirtIOSound,
};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// 自检音频率（Hz）。
pub const TONE_HZ: u32 = 440;
/// 自检音采样率。
pub const TONE_RATE: u32 = 48_000;
/// 自检音时长（毫秒）。
pub const TONE_MILLIS: u32 = 250;
/// 允许请求的频率范围（Hz）。
pub const TONE_MIN_HZ: u32 = 100;
/// 允许请求的频率上限（Hz）。
pub const TONE_MAX_HZ: u32 = 8_000;
/// 允许请求的时长范围（毫秒）。
pub const TONE_MIN_MILLIS: u32 = 50;
/// 允许请求的时长上限（毫秒）。
pub const TONE_MAX_MILLIS: u32 = 2_000;
/// 单声道。
const TONE_CHANNELS: u8 = 1;
/// 一帧字节数：单声道 S16。
const FRAME_BYTES: u32 = 2;

/// virtio 音频设备。
pub struct Sound {
    sound: Mutex<VirtIOSound<HalImpl, PciTransport>>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Sound {
    pub(crate) fn new(sound: VirtIOSound<HalImpl, PciTransport>) -> Self {
        Self {
            sound: Mutex::new(sound),
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        }
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// jack 数量（物理插孔/端口）。
    pub fn jacks(&self) -> u32 {
        self.activate();
        self.sound.lock().jacks()
    }

    /// 输出流数量。
    pub fn output_streams(&self) -> Option<usize> {
        self.activate();
        self.sound
            .lock()
            .output_streams()
            .ok()
            .map(|streams| streams.len())
    }

    /// 播放默认自检音（440 Hz / 250 ms）。
    pub fn tone(&self) -> Option<(usize, u32)> {
        self.tone_at(TONE_HZ, TONE_MILLIS)
    }

    /// 播放指定频率与时长的正弦波，返回 `(输出流数量, 采样率)`。
    ///
    /// 频率与会话长度都先夹到设备自检允许的区间，避免调用方给出零长度或
    /// 超出听域的请求。
    pub fn tone_at(&self, hz: u32, millis: u32) -> Option<(usize, u32)> {
        if !(TONE_MIN_HZ..=TONE_MAX_HZ).contains(&hz)
            || !(TONE_MIN_MILLIS..=TONE_MAX_MILLIS).contains(&millis)
        {
            return None;
        }
        self.activate();
        let mut sound = self.sound.lock();
        let outputs: Vec<u32> = sound.output_streams().ok()?;
        let stream = *outputs.first()?;
        let rates = sound.rates_supported(stream).ok()?;
        if !rates.contains(PcmRates::RATE_48000) {
            return None;
        }
        let formats: PcmFormats = sound.formats_supported(stream).ok()?;
        if !formats.contains(PcmFormats::S16) {
            return None;
        }
        let frames = TONE_RATE * millis / 1000;
        let bytes = frames * FRAME_BYTES;
        // 周期与缓冲都按 10 ms 对齐，给设备留出整周期的缓冲区。
        let period = TONE_RATE / 100 * FRAME_BYTES;
        sound
            .pcm_set_params(
                stream,
                bytes,
                period,
                PcmFeatures::empty(),
                TONE_CHANNELS,
                PcmFormat::S16,
                PcmRate::Rate48000,
            )
            .ok()?;
        sound.pcm_prepare(stream).ok()?;
        sound.pcm_start(stream).ok()?;
        let samples = sine(bytes as usize, hz);
        let played = sound.pcm_xfer(stream, &samples).is_ok();
        let _ = sound.pcm_stop(stream);
        let _ = sound.pcm_release(stream);
        played.then_some((outputs.len(), TONE_RATE))
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&self) {
        self.activate();
        let _ = self.sound.lock().ack_interrupt();
    }
}

/// 生成一段 S16LE 正弦波，幅度为满量程的 1/4。
///
/// `no_std` 没有 `sin`，用二阶递推振荡器：`y[n] = 2cos(w)y[n-1] - y[n-2]`。
/// `w = 2π·hz/48000`，`2cos(w)` 与 `sin(w)` 由频率现算，频率只需落在
/// 100–8000 Hz 的可听区间，展开到二阶即可满足精度。
fn sine(bytes: usize, hz: u32) -> Vec<u8> {
    let w = core::f32::consts::TAU * hz as f32 / TONE_RATE as f32;
    // 二阶泰勒展开在 w ≤ 1.05 rad 时误差 < 1e-3，足够 440/880 Hz 的自检。
    let cos_w = 1.0 - w * w / 2.0 + w * w * w * w / 24.0;
    let sin_w = w - w * w * w / 6.0;
    let two_cos = 2.0 * cos_w;
    let frames = bytes / FRAME_BYTES as usize;
    let amplitude = (i16::MAX / 4) as f32;
    let mut buffer = Vec::with_capacity(bytes);
    let mut previous = 0.0_f32;
    let mut current = amplitude * sin_w;
    for _ in 0..frames {
        let sample = current as i16;
        buffer.extend_from_slice(&sample.to_le_bytes());
        let next = two_cos * current - previous;
        previous = current;
        current = next;
    }
    buffer
}
