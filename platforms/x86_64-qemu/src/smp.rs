//! AP 启动与停泊：由 Limine MP 协议进入 long mode，在线计数在内核池中维护。

use alloc::format;
use alloc::string::String;

use hk_boot::ApCpu;
use hk_kernel::AP_POOL;

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
    let online = AP_POOL.wait(started, hk_platform_x86_64::timing::rdtsc, timeout);
    if online < started {
        return Err(format!("AP 启动超时：{online}/{started}"));
    }
    Ok((started, online))
}

unsafe extern "C" fn ap_entry(_cpu: *mut ApCpu) -> ! {
    AP_POOL.park()
}
