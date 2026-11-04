//! AP 启动与停泊：由 Limine MP 协议进入 long mode，在线计数在内核池中维护。

use alloc::format;
use alloc::string::String;
use core::sync::atomic::Ordering;

use hk_boot::ApCpu;
use hk_kernel::AP_POOL;
use hk_platform_x86_64::cpu;

/// 启动 AP、等待其初始化完成，并发送一次 IPI 自检。
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

    let ids = AP_POOL.ids();
    if let Some(&apic_id) = ids.first() {
        if apic_id > u32::from(u8::MAX) {
            return Err(format!("APIC 编号超出 xAPIC 范围：{apic_id}"));
        }

        let before = cpu::ipi_count();
        cpu::send_ipi(apic_id as u8, cpu::IPI_VECTOR);
        let begin = hk_platform_x86_64::timing::rdtsc();
        while cpu::ipi_count() == before
            && hk_platform_x86_64::timing::rdtsc().wrapping_sub(begin) < timeout
        {
            core::hint::spin_loop();
        }
        if cpu::ipi_count() == before {
            return Err(String::from("IPI 自检超时"));
        }
    }

    Ok((started, online))
}

unsafe extern "C" fn ap_entry(info: *mut ApCpu) -> ! {
    // SAFETY: Limine 传入的 CPU 描述在入口期间有效；`extra` 由 `start_aps` 写入。
    let (lapic_id, cpu_index) = unsafe {
        let cpu = &*info;
        (cpu.lapic_id, cpu.extra.load(Ordering::Acquire) as usize)
    };
    cpu::init_ap(cpu_index);
    cpu::start_ap_tick();
    AP_POOL.mark_online(lapic_id);
    crate::runtime::run_ap(cpu_index)
}
