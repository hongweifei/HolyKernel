use super::*;

impl Kernel {
    /// 发送消息；消息入队或直接交付后立即返回，不阻塞发送者。
    ///
    /// Endpoint 自带消息队列。若发送方在入队后阻塞等待接收方取走，两个服务互相
    /// 发送时就可能都停在发送上（例如网络服务发送轮询请求时，驱动正把收到的帧
    /// 发回），形成双向死锁。需要应答的同步请求使用 [`Self::call`]。
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
            SendOutcome::Queued => {}
            SendOutcome::Full => return Err(KernelError::QueueFull),
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
        self.recv_from(thread, endpoint_handle, None)
    }

    /// 接收指定发送者的消息；`expected` 为 `None` 时接收任意发送者。
    pub fn recv_from(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
        expected: Option<TcbId>,
    ) -> Result<Option<Received>, KernelError> {
        self.recv_from_inner(thread, endpoint_handle, expected, true)
    }

    /// 非阻塞接收；没有消息时返回 `None`，且不会阻塞调用线程。
    pub fn try_recv_from(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
        expected: Option<TcbId>,
    ) -> Result<Option<Received>, KernelError> {
        self.recv_from_inner(thread, endpoint_handle, expected, false)
    }

    fn recv_from_inner(
        &self,
        thread: TcbId,
        endpoint_handle: Handle,
        expected: Option<TcbId>,
        block_when_empty: bool,
    ) -> Result<Option<Received>, KernelError> {
        if let Some(delivery) = self
            .threads
            .lock()
            .get_mut(&thread)
            .and_then(|tcb| tcb.take_delivery_from(expected))
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

        let expected = expected.map(|tcb| Waiter::new(tcb.get()));
        // 非阻塞路径不能登记等待者，否则会在端点里留下悬挂的接收者。
        let received = if block_when_empty {
            endpoint.recv_from(Waiter::new(thread.get()), expected)
        } else {
            endpoint.try_recv_from(expected)
        };
        match received {
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
                if block_when_empty
                    && let Err(error) = self.block(
                        thread,
                        BlockReason::Recv {
                            endpoint: endpoint_id,
                        },
                    )
                {
                    // 阻塞失败时撤销刚登记的接收者，避免悬挂等待。
                    endpoint.cancel_receiver(Waiter::new(thread.get()));
                    return Err(error);
                }
                self.record(thread.get(), Category::Ipc, TRACE_RECV, endpoint_id.get());
                Ok(None)
            }
        }
    }

    /// 取消线程在端点上的接收等待。
    pub fn cancel_recv(&self, thread: TcbId, endpoint_handle: Handle) -> Result<(), KernelError> {
        let (object, _) =
            self.resolve(thread, endpoint_handle, Rights::READ, ObjectKind::Endpoint)?;
        let endpoint = object.endpoint().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Endpoint,
        })?;
        endpoint.cancel_receiver(Waiter::new(thread.get()));
        Ok(())
    }

    /// 取消线程在端点上的发送等待。
    pub fn cancel_send(&self, thread: TcbId, endpoint_handle: Handle) -> Result<(), KernelError> {
        let (object, _) =
            self.resolve(thread, endpoint_handle, Rights::WRITE, ObjectKind::Endpoint)?;
        let endpoint = object.endpoint().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Endpoint,
        })?;
        endpoint.cancel_sender(Waiter::new(thread.get()));
        Ok(())
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
                        reply: Some(ReplyToken {
                            caller: thread,
                            sequence,
                        }),
                    });
                }
                self.wake(receiver);
            }
            SendOutcome::Queued => {}
            SendOutcome::Full => {
                let mut threads = self.threads.lock();
                if let Some(tcb) = threads.get_mut(&thread) {
                    tcb.set_pending_call(None);
                }
                return Err(KernelError::QueueFull);
            }
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

    /// 按对象标识向通知置位；供平台中断分派路径使用。
    pub fn signal_id(&self, id: ObjectId, bits: u64) -> Result<(), KernelError> {
        let object = self
            .objects
            .lock()
            .get(&id)
            .and_then(Weak::upgrade)
            .ok_or(KernelError::StaleObject(id))?;
        let notification = object.notification().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Notification,
        })?;
        if let Some(waiter) = notification.signal(bits) {
            self.wake(TcbId::new(waiter.get()));
        }
        self.record(TcbId::new(0).get(), Category::Ipc, TRACE_SIGNAL, id.get());
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

    pub(crate) fn reply_token(&self, sender: TcbId, endpoint: ObjectId) -> Option<ReplyToken> {
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

    pub(crate) fn transfer_handles(
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
        if !moved.is_empty() {
            self.record(
                sender.get(),
                Category::Handle,
                TRACE_HANDLE_TRANSFER,
                moved.len() as u64,
            );
        }
        Ok(message)
    }
}
