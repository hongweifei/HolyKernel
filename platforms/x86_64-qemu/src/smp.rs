//! AP 启动与停泊：由 Limine MP 协议进入 long mode，记录在线数量。

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicUsize, Ordering};

use hk_boot::ApCpu;

static ONLINE: AtomicUsize = AtomicUsize::new(0);

/// 启动 AP 并等待其停泊；返回（启动数、在线数）。
pub fn start() -> Result<(usize, usize), String> {
    let Some(_info) = hk_boot::smp_info() else {
        return Ok((0, 0));
    };

    let started = hk_boot::start_aps(ap_entry);
    let timeout = match hk_platform_x86_64::timing::tsc_frequency() {
        0 => 1_000_000_000,
        frequency => frequency,
    };
    let begin = hk_platform_x86_64::timing::rdtsc();
    while ONLINE.load(Ordering::Acquire) < started
        && hk_platform_x86_64::timing::rdtsc().wrapping_sub(begin) < timeout
    {
        core::hint::spin_loop();
    }

    let online = ONLINE.load(Ordering::Acquire);
    if online < started {
        return Err(format!("AP 启动超时：{online}/{started}"));
    }
    Ok((started, online))
}

unsafe extern "C" fn ap_entry(_cpu: *mut ApCpu) -> ! {
    ONLINE.fetch_add(1, Ordering::AcqRel);
    loop {
        core::hint::spin_loop();
    }
}
