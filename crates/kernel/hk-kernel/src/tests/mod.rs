//! 内核集成测试。

use hk_device::{DeviceGrant, MmioRange, PciAddress};
use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_handle::{HandleTable, ObjectKind, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{
    Capability, CapabilityKind, Capacity, CapacityId, CapacityKind, FrameBacking, FrameLease,
    ProviderKind, Region, RegionFlags,
};

/// Provider 释放通知标签（与 ABI 一致）。
const PROVIDER_RELEASE: u64 = 0x1205;

use crate::{Kernel, TcbId, ThreadState, TraceCategory, take_provider_releases};
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, DEFAULT_SPEC, millis};

use core::sync::atomic::{AtomicUsize, Ordering};

mod cadence;
mod ipc;
mod objects;
mod scheduling;

static RESCHED_CALLS: AtomicUsize = AtomicUsize::new(0);

fn count_resched(_cpu: usize) {
    RESCHED_CALLS.fetch_add(1, Ordering::SeqCst);
}

fn kernel() -> Kernel {
    Kernel::new()
}

fn thread(kernel: &Kernel) -> TcbId {
    kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务")
}
