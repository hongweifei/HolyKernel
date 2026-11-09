//! 微内核对象、调度与 IPC 分派。
//!
//! 对象表与线程表由自旋锁保护；按对象、调度、IPC、trace 与视图查询拆分。

mod cadence;
mod hmp;
mod ipc;
mod lending;
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
use hk_cadence::{CadenceSpec, CpuMask, IDLE_SPEC};
use hk_device::DeviceGrant;
use hk_handle::{Handle, HandleTable, Object, ObjectId, ObjectKind, Rights};
use hk_ipc::{Message, SendOutcome, Waiter};
use spin::Mutex;

use crate::error::KernelError;
use crate::object::{KernelObject, ObjectPayload};
use crate::sched::Scheduler;
use crate::task::{Task, TaskId, TaskMetrics, TaskState};
use crate::tcb::{BlockReason, PendingCall, PendingDelivery, ReplyToken, Tcb, TcbId, ThreadState};

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
const TRACE_LEND: u32 = 15;
const TRACE_RETURN: u32 = 16;

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
    tasks: Mutex<BTreeMap<TaskId, Arc<spin::Mutex<Task>>>>,
    next_tcb: AtomicU64,
    next_task: AtomicU64,
    scheduler: Mutex<Scheduler>,
    trace: Mutex<TraceRing<TRACE_CAPACITY>>,
    idle: Mutex<BTreeMap<usize, TcbId>>,
    idle_enabled: core::sync::atomic::AtomicBool,
    moment: AtomicU64,
    clock: AtomicU64,
}

impl Kernel {
    /// 构造空内核。
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(BTreeMap::new()),
            threads: Mutex::new(BTreeMap::new()),
            tasks: Mutex::new(BTreeMap::new()),
            next_tcb: AtomicU64::new(1),
            next_task: AtomicU64::new(1),
            scheduler: Mutex::new(Scheduler::new()),
            trace: Mutex::new(TraceRing::new()),
            idle: Mutex::new(BTreeMap::new()),
            idle_enabled: core::sync::atomic::AtomicBool::new(false),
            moment: AtomicU64::new(0),
            clock: AtomicU64::new(0),
        }
    }

    /// 返回调度时钟（契约 tick）。
    pub fn now(&self) -> u64 {
        self.clock.load(Ordering::Relaxed)
    }

    /// 设置调度时钟。
    pub fn set_clock(&self, now: u64) {
        self.clock.store(now, Ordering::Relaxed);
    }

    /// 设置调度器可见的在线 CPU 数。
    pub fn set_online_cpus(&self, online: usize) {
        self.scheduler.lock().set_online(online);
    }

    /// 为每个在线 CPU 建立 idle TCB。
    pub fn enable_idle_cpus(&self, online: usize) {
        self.set_online_cpus(online);
        self.idle_enabled
            .store(true, core::sync::atomic::Ordering::Release);
        for cpu in 0..online.min(hk_hal::MAX_CPUS) {
            self.ensure_idle(cpu);
        }
    }

    /// 为指定 CPU 建立 idle TCB；幂等。
    fn ensure_idle(&self, cpu: usize) {
        if !self
            .idle_enabled
            .load(core::sync::atomic::Ordering::Acquire)
            || cpu >= self.scheduler.lock().online()
            || self.idle.lock().contains_key(&cpu)
        {
            return;
        }
        let spec = CadenceSpec {
            affinity: CpuMask::single(cpu as u32).expect("CPU 索引合法"),
            migratable: false,
            ..IDLE_SPEC
        };
        let Ok(thread) = self.spawn_task_parked(HandleTable::new(0), spec) else {
            return;
        };
        let _ = self.activate(thread);
        self.idle.lock().insert(cpu, thread);
    }

    /// 判断 TCB 是否为 idle TCB。
    pub fn is_idle(&self, thread: TcbId) -> bool {
        self.idle.lock().values().any(|idle| *idle == thread)
    }

    /// 返回某个 CPU 的 idle TCB。
    pub fn idle_thread(&self, cpu: usize) -> Option<TcbId> {
        self.idle.lock().get(&cpu).copied()
    }

    /// 返回 Task；不存在时为 `None`。
    pub(crate) fn task(&self, id: TaskId) -> Option<Arc<spin::Mutex<Task>>> {
        self.tasks.lock().get(&id).cloned()
    }

    /// 返回 TCB 所属 Task。
    pub fn task_of(&self, thread: TcbId) -> Option<TaskId> {
        self.threads.lock().get(&thread).map(Tcb::task)
    }

    /// 返回 Task 的只读计量快照。
    pub fn task_metrics(&self, thread: TcbId) -> Option<TaskMetrics> {
        let task = self.task(self.task_of(thread)?)?;
        Some(task.lock().metrics())
    }

    /// 返回 Task 的 Cadence 描述；用于 fork、恢复与策略查询。
    pub fn cadence_spec(&self, task: TaskId) -> Option<CadenceSpec> {
        let task = self.task(task)?;
        let cadence = task.lock().cadence();
        let cadence = cadence.lock();
        Some(CadenceSpec {
            class: cadence.class(),
            weight: cadence.weight().get(),
            budget: cadence.budget(),
            period: cadence.period(),
            deadline: cadence.deadline(),
            latency: cadence.latency(),
            affinity: cadence.affinity(),
            migratable: cadence.migratable(),
            lending: cadence.lending(),
        })
    }

    /// 返回句柄指向的对象类别。
    pub fn handle_kind(&self, thread: TcbId, handle: Handle) -> Option<ObjectKind> {
        let threads = self.threads.lock();
        let tcb = threads.get(&thread)?;
        tcb.handles()
            .resolve(handle, Rights::READ)
            .ok()
            .map(|resolved| resolved.kind())
    }

    /// 返回 Task 的 Cadence/Meter 句柄，供 Root/Tempo/Debug 查询。
    pub fn grant_task(
        &self,
        thread: TcbId,
        task: TaskId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let task = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Task(task))),
            rights,
        )
    }

    /// 返回 Task 的 Cadence 句柄。
    pub fn grant_cadence(
        &self,
        thread: TcbId,
        task: TaskId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let cadence = self
            .task(task)
            .ok_or(KernelError::UnknownTask(task))?
            .lock()
            .cadence();
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Cadence(cadence))),
            rights,
        )
    }

    /// 返回 Task 的 Meter 句柄。
    pub fn grant_meter(
        &self,
        thread: TcbId,
        task: TaskId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let meter = self
            .task(task)
            .ok_or(KernelError::UnknownTask(task))?
            .lock()
            .meter();
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Meter(meter))),
            rights,
        )
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}
