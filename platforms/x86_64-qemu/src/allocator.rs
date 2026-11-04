//! 内核堆：静态 1 MiB 线性分配。

use hk_kernel::FreeHeap;

/// 内核堆大小。
const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: FreeHeap<HEAP_SIZE> = FreeHeap::new();
