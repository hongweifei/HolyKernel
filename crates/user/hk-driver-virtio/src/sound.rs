//! virtio-sound 设备封装。
//!
//! 只做播放：取第一个输出流，设置 48 kHz / 单声道 / S16 参数，准备并启动，
//! 送一段正弦波，最后停止并释放。采样参数与格式都先查设备支持位，不支持就
//! 直接失败，不猜设备能力。

use alloc::collections::VecDeque;
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
/// 自检音默认采样率；设备不支持时按 `RATES` 依次回退。
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
/// 可协商的采样率，按优先级排列。
const RATES: [(PcmRate, u32); 6] = [
    (PcmRate::Rate48000, 48_000),
    (PcmRate::Rate44100, 44_100),
    (PcmRate::Rate32000, 32_000),
    (PcmRate::Rate22050, 22_050),
    (PcmRate::Rate16000, 16_000),
    (PcmRate::Rate8000, 8_000),
];

/// 一个周期的字节数（10 ms）。
fn period_bytes(rate: u32) -> u32 {
    rate / 100 * FRAME_BYTES
}

/// 判断设备是否支持某个采样率编码。
fn supports(rates: &PcmRates, code: PcmRate) -> bool {
    match code {
        PcmRate::Rate48000 => rates.contains(PcmRates::RATE_48000),
        PcmRate::Rate44100 => rates.contains(PcmRates::RATE_44100),
        PcmRate::Rate32000 => rates.contains(PcmRates::RATE_32000),
        PcmRate::Rate22050 => rates.contains(PcmRates::RATE_22050),
        PcmRate::Rate16000 => rates.contains(PcmRates::RATE_16000),
        PcmRate::Rate8000 => rates.contains(PcmRates::RATE_8000),
        _ => false,
    }
}
/// 同时在途的周期数；设备队列足够深，留两拍避免首包等待。
const IN_FLIGHT: usize = 2;

/// 一次播放的排队状态。
struct Playback {
    stream: u32,
    /// 周期字节数。
    period: u32,
    /// 总字节数，按周期对齐。
    total: usize,
    /// 已提交到设备的字节数。
    sent: usize,
    /// 振荡器递推状态。
    previous: f32,
    current: f32,
    two_cos: f32,
    /// 在途令牌，按提交顺序完成。
    tokens: VecDeque<u16>,
    /// 请求播放的客户端端点，播放结束时收到 `SOUND_DONE`。
    owner: u64,
    failed: bool,
}

/// virtio 音频设备。
pub struct Sound {
    sound: Mutex<VirtIOSound<HalImpl, PciTransport>>,
    playback: Mutex<Option<Playback>>,
    /// 最近一次播放是否失败；0=空闲/成功，2=失败。
    last_failed: Mutex<bool>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Sound {
    pub(crate) fn new(sound: VirtIOSound<HalImpl, PciTransport>) -> Self {
        Self {
            sound: Mutex::new(sound),
            playback: Mutex::new(None),
            last_failed: Mutex::new(false),
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

    /// 开始一段异步播放，返回 `(输出流数量, 采样率)`。
    ///
    /// 频率与时长的合法范围与设备能力都在这里校验；调用返回后数据由
    /// [`Self::poll`] 逐周期推进，播放完成后向 `owner` 发 `SOUND_DONE`。
    pub fn start_tone(&self, hz: u32, millis: u32, owner: u64) -> Option<(usize, u32)> {
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
        let formats: PcmFormats = sound.formats_supported(stream).ok()?;
        if !formats.contains(PcmFormats::S16) {
            return None;
        }
        let (rate_code, rate) = RATES
            .iter()
            .find(|(code, _)| supports(&rates, *code))
            .copied()?;
        let period = period_bytes(rate);
        let period_frames = rate / 100;
        let frames = (rate * millis / 1000).div_ceil(period_frames) * period_frames;
        let bytes = frames * FRAME_BYTES;
        sound
            .pcm_set_params(
                stream,
                bytes,
                period,
                PcmFeatures::empty(),
                TONE_CHANNELS,
                PcmFormat::S16,
                rate_code,
            )
            .ok()?;
        sound.pcm_prepare(stream).ok()?;
        sound.pcm_start(stream).ok()?;
        let w = core::f32::consts::TAU * hz as f32 / rate as f32;
        let cos_w = 1.0 - w * w / 2.0 + w * w * w * w / 24.0;
        let sin_w = w - w * w * w / 6.0;
        let mut playback = Playback {
            stream,
            period,
            total: bytes as usize,
            sent: 0,
            previous: 0.0,
            current: (i16::MAX / 4) as f32 * sin_w,
            two_cos: 2.0 * cos_w,
            tokens: VecDeque::new(),
            owner,
            failed: false,
        };
        pump(&mut sound, &mut playback, period);
        *self.last_failed.lock() = false;
        *self.playback.lock() = Some(playback);
        Some((outputs.len(), rate))
    }

    /// 推进播放：回收完成周期并补交新周期；播完返回 `(owner, 是否成功)`。
    pub fn poll(&self) -> Option<(u64, bool)> {
        self.activate();
        let mut playback = self.playback.lock();
        let state = playback.as_mut()?;
        let mut sound = self.sound.lock();
        let period = state.period;
        while let Some(&token) = state.tokens.front() {
            match sound.pcm_xfer_ok(token) {
                Ok(()) => {
                    state.tokens.pop_front();
                }
                Err(virtio_drivers::Error::NotReady) => break,
                Err(_) => {
                    state.failed = true;
                    state.tokens.clear();
                    break;
                }
            }
        }
        if !state.failed {
            pump(&mut sound, state, period);
        }
        let finished = state.failed || (state.sent >= state.total && state.tokens.is_empty());
        if !finished {
            return None;
        }
        let owner = state.owner;
        let ok = !state.failed;
        *self.last_failed.lock() = state.failed;
        let stream = state.stream;
        let _ = sound.pcm_stop(stream);
        let _ = sound.pcm_release(stream);
        *playback = None;
        Some((owner, ok))
    }

    /// 播放状态：1=播放中，0=空闲/已播完，2=失败。
    pub fn status(&self) -> u32 {
        if self.playback.lock().is_some() {
            1
        } else if *self.last_failed.lock() {
            2
        } else {
            0
        }
    }

    /// 中止当前播放并释放流。
    pub fn stop(&self) -> bool {
        self.activate();
        let mut playback = self.playback.lock();
        let Some(state) = playback.take() else {
            return false;
        };
        let mut sound = self.sound.lock();
        let _ = sound.pcm_stop(state.stream);
        let _ = sound.pcm_release(state.stream);
        true
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&self) {
        self.activate();
        let _ = self.sound.lock().ack_interrupt();
    }
}

/// 尽力把后续周期提交到设备，直到在途数达到上限或数据送完。
fn pump(sound: &mut VirtIOSound<HalImpl, PciTransport>, state: &mut Playback, period: u32) {
    let mut buffer = [0_u8; MAX_PERIOD];
    while !state.failed && state.tokens.len() < IN_FLIGHT && state.sent < state.total {
        let length = (state.total - state.sent).min(period as usize);
        fill_period(
            &mut buffer[..length],
            &mut state.previous,
            &mut state.current,
            state.two_cos,
        );
        match sound.pcm_xfer_nb(state.stream, &buffer[..length]) {
            Ok(token) => {
                state.tokens.push_back(token);
                state.sent += length;
            }
            Err(virtio_drivers::Error::QueueFull) => break,
            Err(_) => {
                state.failed = true;
                break;
            }
        }
    }
}

/// 单个周期缓冲上限：48 kHz / 10 ms / 单声道 S16。
const MAX_PERIOD: usize = (48_000 / 100) * FRAME_BYTES as usize;

/// 按递推振荡器填充一个周期的 S16LE 正弦，幅度为满量程的 1/4。
fn fill_period(buffer: &mut [u8], previous: &mut f32, current: &mut f32, two_cos: f32) {
    for frame in buffer.as_chunks_mut::<{ FRAME_BYTES as usize }>().0 {
        let sample = *current as i16;
        frame.copy_from_slice(&sample.to_le_bytes());
        let next = two_cos * *current - *previous;
        *previous = *current;
        *current = next;
    }
}
