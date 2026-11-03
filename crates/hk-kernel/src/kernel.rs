//! 内核对象表、线程、调度与 IPC 分派。
//!
//! 内核对象表与线程表由自旋锁保护；Hosted 目标在单线程中驱动调度，跨 CPU 的
//! 原子性随 SMP 支持一并引入。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{Mmu, PhysRange};
use hk_handle::{Handle, HandleTable, Object, ObjectId, ObjectKind, Rights};
use hk_ipc::{Message, SendOutcome, Waiter};
use hk_mem::{Untyped, VSpace};
use hk_trace::{Category, TraceRing};
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

    /// 在指定线程的句柄表中创建 Untyped 对象。
    pub fn create_untyped(
        &self,
        thread: TcbId,
        region: PhysRange,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Untyped(Mutex::new(Untyped::new(region)));
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 在指定线程的句柄表中创建端点。
    pub fn create_endpoint(&self, thread: TcbId, rights: Rights) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Endpoint(hk_ipc::Endpoint::new());
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 在指定线程的句柄表中创建通知。
    pub fn create_notification(
        &self,
        thread: TcbId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Notification(hk_ipc::Notification::new());
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 在指定线程的句柄表中创建地址空间。
    pub fn create_vspace(
        &self,
        thread: TcbId,
        mmu: &dyn Mmu,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::VSpace(VSpace::new(mmu.create_vspace()));
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 从 Untyped 切分页帧并授予句柄。
    pub fn allocate_frame(
        &self,
        thread: TcbId,
        untyped: Handle,
        len: usize,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, untyped, Rights::WRITE, ObjectKind::Untyped)?;
        let frame = {
            let untyped = object.untyped().ok_or(KernelError::WrongKind {
                expected: ObjectKind::Untyped,
            })?;
            untyped.lock().retype_frame(len)?
        };
        let header = frame.header_arc().clone();
        let payload = ObjectPayload::Frame(frame);
        self.register(
            thread,
            Arc::new(KernelObject::with_header(header, payload)),
            rights,
        )
    }

    /// 把已有对象的句柄授予指定线程。
    pub fn grant(
        &self,
        thread: TcbId,
        object: ObjectId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let target = self.upgrade(object)?;
        self.register(thread, target, rights)
    }

    /// 创建线程；线程获得独立的句柄表。
    pub fn spawn_thread(&self, handles: HandleTable, priority: Priority) -> TcbId {
        let id = TcbId::new(self.next_tcb.fetch_add(1, Ordering::Relaxed));
        self.threads
            .lock()
            .insert(id, Tcb::new(id, priority, handles));
        self.scheduler.lock().enqueue(id, priority);
        self.record(id.get(), Category::Sched, TRACE_THREAD_SPAWN, 0);
        id
    }

    /// 取下一个可运行线程；当前线程按同优先级轮转回到就绪队列。
    pub fn schedule(&self) -> Option<TcbId> {
        let current = self.scheduler.lock().take_current();
        if let Some(current) = current {
            let requeue = {
                let mut threads = self.threads.lock();
                match threads.get_mut(&current) {
                    Some(tcb) if tcb.state() == ThreadState::Running => {
                        tcb.set_state(ThreadState::Runnable);
                        Some(tcb.priority())
                    }
                    _ => None,
                }
            };
            if let Some(priority) = requeue {
                self.scheduler.lock().enqueue(current, priority);
            }
        }

        let next = self.scheduler.lock().pick()?;
        {
            let mut threads = self.threads.lock();
            if let Some(tcb) = threads.get_mut(&next) {
                tcb.set_state(ThreadState::Running);
            }
        }
        self.record(next.get(), Category::Sched, TRACE_DISPATCH, 0);
        Some(next)
    }

    /// 终止线程；句柄表随 TCB 一起释放。
    pub fn exit(&self, thread: TcbId) -> Result<(), KernelError> {
        let tcb = self
            .threads
            .lock()
            .remove(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        drop(tcb);
        {
            let mut scheduler = self.scheduler.lock();
            scheduler.remove(thread);
            scheduler.clear_current_if(thread);
        }
        self.record(thread.get(), Category::Sched, TRACE_EXIT, 0);
        Ok(())
    }

    /// 发送消息；没有接收者时发送方阻塞。
    pub fn send(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
        message: Message,
    ) -> Result<(), KernelError> {
        let (object, endpoint_id) =
            self.resolve(thread, endpoint_handle, Rights::WRITE, ObjectKind::Endpoint)?;
        let endpoint = object.endpoint().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Endpoint,
        })?;

        match endpoint.send(Waiter::new(thread.get()), message) {
            SendOutcome::Delivered { receiver, message } => {
                let receiver = TcbId::new(receiver.get());
                let message = self.transfer_handles(thread, receiver, message)?;
                {
                    let mut threads = self.threads.lock();
                    let target = threads
                        .get_mut(&receiver)
                        .ok_or(KernelError::UnknownThread(receiver))?;
                    target.push_delivery(PendingDelivery {
                        sender: thread,
                        message,
                        reply: None,
                    });
                }
                self.wake(receiver);
            }
            SendOutcome::Queued => {
                self.block(
                    thread,
                    BlockReason::Send {
                        endpoint: endpoint_id,
                    },
                )?;
            }
        }
        self.record(thread.get(), Category::Ipc, TRACE_SEND, endpoint_id.get());
        Ok(())
    }

    /// 接收消息；没有消息时接收方阻塞并返回 `None`。
    pub fn recv(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
    ) -> Result<Option<Received>, KernelError> {
        if let Some(delivery) = self
            .threads
            .lock()
            .get_mut(&thread)
            .and_then(Tcb::take_delivery)
        {
            return Ok(Some(Received {
                sender: delivery.sender,
                message: delivery.message,
                reply: delivery.reply,
            }));
        }

        let (object, endpoint_id) =
            self.resolve(thread, endpoint_handle, Rights::READ, ObjectKind::Endpoint)?;
        let endpoint = object.endpoint().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Endpoint,
        })?;

        match endpoint.recv(Waiter::new(thread.get())) {
            Some((sender, message)) => {
                let sender = TcbId::new(sender.get());
                let reply = self.reply_token(sender, endpoint_id);
                let message = self.transfer_handles(sender, thread, message)?;
                if reply.is_none() {
                    self.wake(sender);
                }
                self.record(thread.get(), Category::Ipc, TRACE_RECV, endpoint_id.get());
                Ok(Some(Received {
                    sender,
                    message,
                    reply,
                }))
            }
            None => {
                self.block(
                    thread,
                    BlockReason::Recv {
                        endpoint: endpoint_id,
                    },
                )?;
                self.record(thread.get(), Category::Ipc, TRACE_RECV, endpoint_id.get());
                Ok(None)
            }
        }
    }

    /// 调用端点；调用方阻塞，服务端回复后由 [`Kernel::take_reply`] 取回结果。
    pub fn call(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
        message: Message,
    ) -> Result<(), KernelError> {
        let (object, endpoint_id) =
            self.resolve(thread, endpoint_handle, Rights::WRITE, ObjectKind::Endpoint)?;
        let endpoint = object.endpoint().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Endpoint,
        })?;

        let sequence = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            let sequence = tcb.next_sequence();
            tcb.set_pending_call(Some(PendingCall {
                endpoint: endpoint_id,
                sequence,
                reply: None,
                answered: false,
            }));
            sequence
        };

        if let SendOutcome::Delivered { receiver, message } =
            endpoint.send(Waiter::new(thread.get()), message)
        {
            let receiver = TcbId::new(receiver.get());
            let message = self.transfer_handles(thread, receiver, message)?;
            {
                let mut threads = self.threads.lock();
                let target = threads
                    .get_mut(&receiver)
                    .ok_or(KernelError::UnknownThread(receiver))?;
                target.push_delivery(PendingDelivery {
                    sender: thread,
                    message,
                    reply: Some(ReplyToken {
                        caller: thread,
                        sequence,
                    }),
                });
            }
            self.wake(receiver);
        }

        self.block(
            thread,
            BlockReason::Call {
                endpoint: endpoint_id,
            },
        )?;
        self.record(thread.get(), Category::Ipc, TRACE_CALL, endpoint_id.get());
        Ok(())
    }

    /// 回复调用；令牌校验调用方与调用序号。
    pub fn reply(
        &self,
        thread: TcbId,
        token: ReplyToken,
        message: Message,
    ) -> Result<(), KernelError> {
        let message = self.transfer_handles(thread, token.caller, message)?;
        {
            let mut threads = self.threads.lock();
            let caller = threads
                .get_mut(&token.caller)
                .ok_or(KernelError::UnknownThread(token.caller))?;
            let pending = caller.pending_call_mut().ok_or(KernelError::InvalidReply)?;
            if pending.sequence != token.sequence || pending.answered {
                return Err(KernelError::InvalidReply);
            }
            pending.reply = Some(message);
            pending.answered = true;
        }
        self.wake(token.caller);
        self.record(thread.get(), Category::Ipc, TRACE_REPLY, token.caller.get());
        Ok(())
    }

    /// 取走调用回复。
    pub fn take_reply(&self, thread: TcbId) -> Option<Message> {
        self.threads.lock().get_mut(&thread)?.take_reply()
    }

    /// 置位通知；有等待者时唤醒。
    pub fn signal(
        &self,
        thread: TcbId,
        notification_handle: Handle,
        bits: u64,
    ) -> Result<(), KernelError> {
        let (object, notification_id) = self.resolve(
            thread,
            notification_handle,
            Rights::WRITE,
            ObjectKind::Notification,
        )?;
        let notification = object.notification().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Notification,
        })?;

        if let Some(waiter) = notification.signal(bits) {
            self.wake(TcbId::new(waiter.get()));
        }
        self.record(
            thread.get(),
            Category::Ipc,
            TRACE_SIGNAL,
            notification_id.get(),
        );
        Ok(())
    }

    /// 等待通知位；没有待处理位时阻塞并返回 `None`。
    pub fn wait(
        &self,
        thread: TcbId,
        notification_handle: Handle,
        mask: u64,
    ) -> Result<Option<u64>, KernelError> {
        let (object, notification_id) = self.resolve(
            thread,
            notification_handle,
            Rights::READ,
            ObjectKind::Notification,
        )?;
        let notification = object.notification().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Notification,
        })?;

        match notification.wait(Waiter::new(thread.get()), mask) {
            Some(bits) => Ok(Some(bits)),
            None => {
                self.block(
                    thread,
                    BlockReason::Notification {
                        notification: notification_id,
                        mask,
                    },
                )?;
                Ok(None)
            }
        }
    }

    /// 返回线程状态。
    pub fn state_of(&self, thread: TcbId) -> Option<ThreadState> {
        self.threads.lock().get(&thread).map(Tcb::state)
    }

    /// 返回线程数量。
    pub fn thread_count(&self) -> usize {
        self.threads.lock().len()
    }

    /// 返回存活对象数量。
    pub fn live_objects(&self) -> usize {
        self.objects
            .lock()
            .values()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    /// 返回就绪线程数量。
    pub fn runnable(&self) -> usize {
        self.scheduler.lock().runnable()
    }

    /// 返回句柄表中的在用句柄数。
    pub fn handles_live(&self, thread: TcbId) -> Option<usize> {
        self.threads
            .lock()
            .get(&thread)
            .map(|tcb| tcb.handles().live())
    }

    /// 关闭线程句柄表中的一个句柄。
    pub fn close_handle(&self, thread: TcbId, handle: Handle) -> Result<(), KernelError> {
        let mut threads = self.threads.lock();
        let tcb = threads
            .get_mut(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        tcb.handles_mut().close(handle)?;
        Ok(())
    }

    /// 返回句柄指向的对象标识。
    pub fn handle_id(&self, thread: TcbId, handle: Handle) -> Result<ObjectId, KernelError> {
        let threads = self.threads.lock();
        let tcb = threads
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        Ok(tcb.handles().resolve(handle, Rights::empty())?.id())
    }

    /// 返回 trace 事件数量。
    pub fn trace_len(&self) -> usize {
        self.trace.lock().len()
    }

    /// 清理已析构对象的注册表项，返回清理数量。
    pub fn collect_garbage(&self) -> usize {
        let mut objects = self.objects.lock();
        let before = objects.len();
        objects.retain(|_, weak| weak.strong_count() > 0);
        before - objects.len()
    }

    /// 生成内核状态报告。
    ///
    /// panic dump 与测试断言使用同一份报告，保证故障现场可复现。
    pub fn dump(&self) -> String {
        let mut text = String::new();
        let runnable = self.runnable();
        let _ = writeln!(
            text,
            "kernel: threads={} objects={} runnable={}",
            self.thread_count(),
            self.live_objects(),
            runnable
        );

        for tcb in self.threads.lock().values() {
            let _ = writeln!(
                text,
                "  tcb={} state={:?} priority={} handles={} blocked={:?} pending={}",
                tcb.id().get(),
                tcb.state(),
                tcb.priority().level(),
                tcb.handles().live(),
                tcb.blocked(),
                tcb.pending_deliveries()
            );
        }

        let trace = self.trace.lock();
        let _ = writeln!(text, "trace: events={}", trace.len());
        for event in trace.latest(8) {
            let _ = writeln!(
                text,
                "  seq={} cat={} thread={} code={} arg={:#x}",
                event.sequence,
                event.category.name(),
                event.thread,
                event.code,
                event.argument
            );
        }
        text
    }

    fn register(
        &self,
        thread: TcbId,
        object: Arc<KernelObject>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let handle = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.handles_mut()
                .insert(Arc::clone(&object) as Arc<dyn Object>, rights)?
        };
        self.objects
            .lock()
            .insert(object.header().id(), Arc::downgrade(&object));
        self.record(
            thread.get(),
            Category::Handle,
            TRACE_HANDLE_GRANT,
            object.header().id().get(),
        );
        Ok(handle)
    }

    fn resolve(
        &self,
        thread: TcbId,
        handle: Handle,
        required: Rights,
        expected: ObjectKind,
    ) -> Result<(Arc<KernelObject>, ObjectId), KernelError> {
        let id = {
            let threads = self.threads.lock();
            let tcb = threads
                .get(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.handles().resolve_kind(handle, required, expected)?.id()
        };
        Ok((self.upgrade(id)?, id))
    }

    fn upgrade(&self, id: ObjectId) -> Result<Arc<KernelObject>, KernelError> {
        self.objects
            .lock()
            .get(&id)
            .and_then(Weak::upgrade)
            .ok_or(KernelError::StaleObject(id))
    }

    fn block(&self, thread: TcbId, reason: BlockReason) -> Result<(), KernelError> {
        {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.set_state(ThreadState::Blocked);
            tcb.set_blocked(Some(reason));
        }
        let mut scheduler = self.scheduler.lock();
        scheduler.remove(thread);
        scheduler.clear_current_if(thread);
        drop(scheduler);
        self.record(thread.get(), Category::Sched, TRACE_BLOCK, 0);
        Ok(())
    }

    fn wake(&self, thread: TcbId) {
        let priority = {
            let mut threads = self.threads.lock();
            let Some(tcb) = threads.get_mut(&thread) else {
                return;
            };
            if tcb.state() != ThreadState::Blocked {
                return;
            }
            tcb.set_state(ThreadState::Runnable);
            tcb.set_blocked(None);
            tcb.priority()
        };
        self.scheduler.lock().enqueue(thread, priority);
        self.record(thread.get(), Category::Sched, TRACE_WAKE, 0);
    }

    fn reply_token(&self, sender: TcbId, endpoint: ObjectId) -> Option<ReplyToken> {
        let threads = self.threads.lock();
        let call = threads.get(&sender)?.pending_call()?;
        if call.endpoint != endpoint {
            return None;
        }
        Some(ReplyToken {
            caller: sender,
            sequence: call.sequence,
        })
    }

    fn transfer_handles(
        &self,
        sender: TcbId,
        receiver: TcbId,
        mut message: Message,
    ) -> Result<Message, KernelError> {
        let handles: Vec<Handle> = message.handles.iter().flatten().copied().collect();
        if handles.is_empty() {
            return Ok(message);
        }

        {
            let threads = self.threads.lock();
            let target = threads
                .get(&receiver)
                .ok_or(KernelError::UnknownThread(receiver))?;
            let available = target
                .handles()
                .quota()
                .saturating_sub(target.handles().live());
            if available < handles.len() {
                return Err(hk_handle::HandleError::NoHandles.into());
            }
        }

        let mut moved = Vec::with_capacity(handles.len());
        {
            let mut threads = self.threads.lock();
            let source = threads
                .get_mut(&sender)
                .ok_or(KernelError::UnknownThread(sender))?;
            for handle in &handles {
                moved.push(source.handles_mut().take(*handle, Rights::READ)?);
            }
        }

        let mut granted = Vec::with_capacity(moved.len());
        {
            let mut threads = self.threads.lock();
            let target = threads
                .get_mut(&receiver)
                .ok_or(KernelError::UnknownThread(receiver))?;
            for resolved in &moved {
                let object = Arc::clone(resolved.object());
                granted.push(target.handles_mut().insert(object, resolved.rights())?);
            }
        }

        message.handles = [None; hk_ipc::MAX_MESSAGE_HANDLES];
        for (index, handle) in granted.into_iter().enumerate() {
            message.handles[index] = Some(handle);
        }
        Ok(message)
    }

    fn record(&self, thread: u64, category: Category, code: u32, argument: u64) {
        let moment = self.moment.fetch_add(1, Ordering::Relaxed) + 1;
        self.trace
            .lock()
            .record(moment, 0, thread, category, code, argument);
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}
