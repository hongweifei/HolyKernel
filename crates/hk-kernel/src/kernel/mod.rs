//! 微内核对象、调度与 IPC 分派。
//!
//! 对象表与线程表由自旋锁保护；按对象、调度、IPC、trace 与视图查询拆分。

mod hmp;
mod ipc;
mod object;
mod sched;
mod state;
mod tracing;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::trace::{Category, TraceRing};
use hk_hal::{DeviceGrant, Mmu, PhysRange};
use hk_handle::{Handle, HandleTable, Object, ObjectId, ObjectKind, Rights};
use hk_ipc::{Message, SendOutcome, Waiter};
use hk_mem::{Untyped, VSpace};
use spin::Mutex;

use crate::error::KernelError;
use crate::object::{KernelObject, ObjectPayload};
use crate::sched::Scheduler;
use crate::tcb::{
    BlockReason, PendingCall, PendingDelivery, Priority, ReplyToken, Tcb, TcbId, ThreadState,
};

/// trace 环形缓冲容量。
pub const TRACE_CAPACITY: usize = 256;

const TRACE_HANDLE_GRANT: u32 = 1;
const TRACE_THREAD_SPAWN: u32 = 2;
const TRACE_DISPATCH: u32 = 3;
const TRACE_BLOCK: u32 = 4;
const TRACE_WAKE: u32 = 5;
const TRACE_EXIT: u32 = 6;
const TRACE_SEND: u32 = 7;
const TRACE_RECV: u32 = 8;
const TRACE_CALL: u32 = 9;
const TRACE_REPLY: u32 = 10;
const TRACE_SIGNAL: u32 = 11;
const TRACE_HANDLE_CLOSE: u32 = 12;
const TRACE_HANDLE_TRANSFER: u32 = 13;
const TRACE_HANDLE_RELEASE: u32 = 14;

/// 接收方取到的消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// 发送方线程。
    pub sender: TcbId,
    /// 消息本体。
    pub message: Message,
    /// 调用回复令牌；普通发送为 `None`。
    pub reply: Option<ReplyToken>,
}

/// 微内核。
pub struct Kernel {
    objects: Mutex<BTreeMap<ObjectId, Weak<KernelObject>>>,
    threads: Mutex<BTreeMap<TcbId, Tcb>>,
    next_tcb: AtomicU64,
    scheduler: Mutex<Scheduler>,
    trace: Mutex<TraceRing<TRACE_CAPACITY>>,
    moment: AtomicU64,
}

impl Kernel {
    /// 构造空内核。
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(BTreeMap::new()),
            threads: Mutex::new(BTreeMap::new()),
            next_tcb: AtomicU64::new(1),
            scheduler: Mutex::new(Scheduler::new()),
            trace: Mutex::new(TraceRing::new()),
            moment: AtomicU64::new(0),
        }
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}
