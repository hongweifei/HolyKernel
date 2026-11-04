use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 命中调度 tick 时保存现场；返回是否切换任务。
    pub(crate) fn send(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
        transfer: bool,
    ) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        let transferred = if transfer {
            let number = frame.arg3() as usize;
            match self.handle_at(index, number) {
                Some(transferred) => Some((number, transferred)),
                None => {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
            }
        } else {
            None
        };

        let mut words = [0; 8];
        words[0] = frame.arg2();
        let mut message = Message::new(frame.arg1()).with_words(words);
        if let Some((_, transferred)) = transferred {
            message = message.with_handles([Some(transferred), None, None, None]);
        }

        match self.kernel.send(tcb, handle, message) {
            Ok(()) => {
                if self.kernel.state_of(tcb) == Some(ThreadState::Blocked) {
                    let mut context = *frame;
                    context.set_result(0);
                    self.tasks[index].context = context;
                    self.tasks[index].pending = Pending::Send {
                        endpoint: frame.arg0(),
                        handle,
                    };
                    Switch::Block
                } else {
                    if let Some((number, _)) = transferred {
                        self.clear_handle(index, number);
                    }
                    frame.set_result(0);
                    Switch::Continue
                }
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn recv(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        match self.kernel.recv(tcb, handle) {
            Ok(Some(received)) => {
                self.tasks[index].reply = received.reply;
                let sender = self.endpoint_of(received.sender);
                let handle = self.install_handle(index, &received.message);
                frame.set_result(received.message.label);
                frame.set_word(received.message.words[0]);
                frame.set_sender(sender);
                frame.set_handle(handle);
                Switch::Continue
            }
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::Recv { handle };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn notify(&self, index: usize, tcb: TcbId, frame: &mut C) -> u64 {
        let number = frame.arg0() as usize;
        let Some(handle) = self.notification_handle(index, number) else {
            return u64::MAX;
        };
        match self.kernel.signal(tcb, handle, frame.arg1()) {
            Ok(()) => 0,
            Err(_) => u64::MAX,
        }
    }

    pub(crate) fn wait_notify(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let mask = frame.arg1();
        let Some(handle) = self.notification_handle(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        match self.kernel.wait(tcb, handle, mask) {
            Ok(Some(bits)) => {
                frame.set_result(bits);
                Switch::Continue
            }
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::Notify { handle, mask };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn call(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        let mut words = [0; 8];
        words[0] = frame.arg2();
        let message = Message::new(frame.arg1()).with_words(words);
        match self.kernel.call(tcb, handle, message) {
            Ok(()) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::Call {
                    endpoint: endpoint as u64,
                    started: self.platform.timestamp(),
                };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn reply(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> u64 {
        let Some(token) = self.tasks[index].reply.take() else {
            return u64::MAX;
        };
        let mut words = [0; 8];
        words[0] = frame.arg1();
        let message = Message::new(frame.arg0()).with_words(words);
        match self.kernel.reply(tcb, token, message) {
            Ok(()) => 0,
            Err(_) => u64::MAX,
        }
    }

    /// 发送带字节载荷的消息。
    pub(crate) fn send_data(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg3() as usize;
        if length > MAX_USER_DATA {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut data = alloc::vec![0_u8; length];
        if length > 0 && !self.platform.read_user(frame.arg2(), &mut data) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }

        let message = Message::new(frame.arg1()).with_data(data);
        match self.kernel.send(tcb, handle, message) {
            Ok(()) => {
                if self.kernel.state_of(tcb) == Some(ThreadState::Blocked) {
                    let mut context = *frame;
                    context.set_result(0);
                    self.tasks[index].context = context;
                    self.tasks[index].pending = Pending::Send {
                        endpoint: frame.arg0(),
                        handle,
                    };
                    Switch::Block
                } else {
                    frame.set_result(0);
                    Switch::Continue
                }
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    /// 接收带字节载荷的消息。
    pub(crate) fn recv_data(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let buffer = frame.arg1();
        let capacity = frame.arg2() as usize;
        if capacity > MAX_USER_DATA {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }

        let expected = self.tcb_of_endpoint(frame.arg3());
        match self.kernel.recv_from(tcb, handle, expected) {
            Ok(Some(received)) => self.deliver_data(index, frame, &received, buffer, capacity),
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::RecvData {
                    handle,
                    buffer,
                    capacity,
                    expected,
                    handle_out: false,
                };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn recv_data_handle(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let buffer = frame.arg1();
        let capacity = frame.arg2() as usize;
        if capacity > MAX_USER_DATA {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let expected = self.tcb_of_endpoint(frame.arg3());
        match self.kernel.recv_from(tcb, handle, expected) {
            Ok(Some(received)) => {
                let number = self.install_handle(index, &received.message);
                self.deliver_data_handle(index, frame, &received, buffer, capacity, number)
            }
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::RecvData {
                    handle,
                    buffer,
                    capacity,
                    expected,
                    handle_out: true,
                };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn deliver_data_handle(
        &mut self,
        index: usize,
        frame: &mut C,
        received: &hk_kernel::Received,
        buffer: u64,
        capacity: usize,
        number: u64,
    ) -> Switch {
        let length = received.message.data.len();
        let copied = length.min(capacity);
        if copied > 0
            && !self
                .platform
                .write_user(buffer, &received.message.data[..copied])
        {
            frame.set_result(u64::MAX);
            frame.set_handle(length as u64);
            return Switch::Continue;
        }

        self.tasks[index].reply = received.reply;
        frame.set_result(received.message.label);
        frame.set_word(received.message.words[0]);
        frame.set_sender(self.endpoint_of(received.sender));
        frame.set_handle((number << 32) | length as u64);
        Switch::Continue
    }

    pub(crate) fn deliver_data(
        &mut self,
        index: usize,
        frame: &mut C,
        received: &hk_kernel::Received,
        buffer: u64,
        capacity: usize,
    ) -> Switch {
        let length = received.message.data.len();
        let copied = length.min(capacity);
        if copied > 0
            && !self
                .platform
                .write_user(buffer, &received.message.data[..copied])
        {
            frame.set_result(u64::MAX);
            frame.set_handle(length as u64);
            return Switch::Continue;
        }

        self.tasks[index].reply = received.reply;
        frame.set_result(received.message.label);
        frame.set_word(received.message.words[0]);
        frame.set_sender(self.endpoint_of(received.sender));
        frame.set_handle(length as u64);
        Switch::Continue
    }
}
