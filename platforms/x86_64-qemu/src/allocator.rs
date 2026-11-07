//! 内核堆：启动期 bootstrap 小堆 + FrameBank 主堆。

use hk_kernel::KernelHeap;

/// 启动期 bootstrap 堆大小。
const BOOTSTRAP_SIZE: usize = 1024 * 1024;

#[global_allocator]
pub(crate) static ALLOCATOR: KernelHeap<BOOTSTRAP_SIZE> = KernelHeap::new();

/// 以 FrameBank 供给的虚拟区间初始化内核主堆。
pub(crate) fn init_dynamic(base: usize, len: usize) {
    ALLOCATOR.init_dynamic(base, len);
}
