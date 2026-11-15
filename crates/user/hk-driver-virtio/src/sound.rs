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
/// 一个周期的字节数（10 ms）。
const PERIOD_BYTES: u32 = TONE_RATE / 100 * FRAME_BYTES;
/// 同时在途的周期数；设备队列足够深，留两拍避免首包等待。
const IN_FLIGHT: usize = 2;

/// 一次播放的排队状态。
struct Playback {
    stream: u32,
    data: Vec<u8>,
    /// 已提交到设备的字节数。
    sent: usize,
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

    /// 播放默认自检音（440 Hz / 250 ms），阻塞到设备收完。
    ///
    /// 只用于启动自检；服务主循环用 [`Self::start_tone`] + [`Self::poll`]，
    /// 不占用消息处理线程。
    pub fn tone(&self) -> Option<(usize, u32)> {
        let started = self.start_tone(TONE_HZ, TONE_MILLIS, 0)?;
        for _ in 0..TONE_RATE * TONE_MAX_MILLIS {
            if self.poll().is_some() {
                return Some(started);
            }
            core::hint::spin_loop();
        }
        let _ = self.stop();
        None
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
        if !rates.contains(PcmRates::RATE_48000) {
            return None;
        }
        let formats: PcmFormats = sound.formats_supported(stream).ok()?;
        if !formats.contains(PcmFormats::S16) {
            return None;
        }
        let mut frames = TONE_RATE * millis / 1000;
        // 设备要求 buffer 是周期的整数倍，帧数向上对齐到 10 ms。
        let period_frames = TONE_RATE / 100;
        frames = frames.div_ceil(period_frames) * period_frames;
        let bytes = frames * FRAME_BYTES;
        sound
            .pcm_set_params(
                stream,
                bytes,
                PERIOD_BYTES,
                PcmFeatures::empty(),
                TONE_CHANNELS,
                PcmFormat::S16,
                PcmRate::Rate48000,
            )
            .ok()?;
        sound.pcm_prepare(stream).ok()?;
        sound.pcm_start(stream).ok()?;
        let mut playback = Playback {
            stream,
            data: sine(bytes as usize, hz),
            sent: 0,
            tokens: VecDeque::new(),
            owner,
            failed: false,
        };
        pump(&mut sound, &mut playback);
        *self.last_failed.lock() = false;
        *self.playback.lock() = Some(playback);
        Some((outputs.len(), TONE_RATE))
    }

    /// 推进播放：回收完成周期并补交新周期；播完返回 `(owner, 是否成功)`。
    pub fn poll(&self) -> Option<(u64, bool)> {
        self.activate();
        let mut playback = self.playback.lock();
        let state = playback.as_mut()?;
        let mut sound = self.sound.lock();
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
            pump(&mut sound, state);
        }
        let finished = state.failed || (state.sent >= state.data.len() && state.tokens.is_empty());
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
fn pump(sound: &mut VirtIOSound<HalImpl, PciTransport>, state: &mut Playback) {
    while !state.failed && state.tokens.len() < IN_FLIGHT && state.sent < state.data.len() {
        let end = (state.sent + PERIOD_BYTES as usize).min(state.data.len());
        match sound.pcm_xfer_nb(state.stream, &state.data[state.sent..end]) {
            Ok(token) => {
                state.tokens.push_back(token);
                state.sent = end;
            }
            Err(virtio_drivers::Error::QueueFull) => break,
            Err(_) => {
                state.failed = true;
                break;
            }
        }
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
