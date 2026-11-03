//! 内核堆：静态 1 MiB 线性分配。

use hk_hal::BumpHeap;

/// 内核堆大小。
const HEAP_SIZE: usize = 1024 * 1024;

#[global_allocator]
static ALLOCATOR: BumpHeap<HEAP_SIZE> = BumpHeap::new();
