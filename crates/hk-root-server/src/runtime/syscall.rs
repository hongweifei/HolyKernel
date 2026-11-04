//! 系统调用分派与 IPC 操作。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 分派一次系统调用。
    pub fn dispatch(&mut self, cpu: usize, frame: &mut C) -> Switch {
        let Some(index) = self.current[cpu] else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let tcb = self.tasks[index].tcb;

        match frame.number() {
            SYS_SEND => self.send(index, tcb, frame, false),
            SYS_SEND_HANDLE => self.send(index, tcb, frame, true),
            SYS_RECV => self.recv(index, tcb, frame),
            SYS_NOTIFY => {
                let result = self.notify(index, tcb, frame);
                frame.set_result(result);
                Switch::Continue
            }
            SYS_WAIT_NOTIFY => self.wait_notify(index, tcb, frame),
            SYS_CALL => self.call(index, tcb, frame),
            SYS_REPLY => {
                let result = self.reply(index, tcb, frame);
                frame.set_result(result);
                Switch::Continue
            }
            SYS_FORK => self.fork_task(index, tcb, frame),
            SYS_YIELD => {
                self.tasks[index].context = *frame;
                self.tasks[index].pending = Pending::None;
                Switch::Block
            }
            SYS_EXIT => {
                let code = frame.arg0();
                let _ = self.kernel.exit(tcb);
                self.tasks[index].exited = true;
                self.tasks[index].exit_code = code;
                Switch::Exit
            }
            SYS_PCI_READ => {
                let address = frame.arg0();
                let bus = (address >> 8) as u8;
                let device = ((address >> 3) & 0x1F) as u8;
                let function = (address & 0x07) as u8;
                frame.set_result(u64::from(self.platform.pci_read(
                    bus,
                    device,
                    function,
                    frame.arg1() as u8,
                )));
                Switch::Continue
            }
            SYS_TICKS => {
                frame.set_result(self.platform.timer_ticks());
                Switch::Continue
            }
            SYS_TSC => {
                frame.set_result(self.platform.timestamp());
                Switch::Continue
            }
            SYS_TSC_FREQ => {
                frame.set_result(self.platform.tsc_frequency());
                Switch::Continue
            }
            SYS_ALLOC => {
                let address = self
                    .allocate(index, frame.arg0() as usize, frame.arg1() as usize)
                    .unwrap_or(0);
                frame.set_result(address);
                Switch::Continue
            }
            SYS_HANDLE_COUNT => {
                frame.set_result(self.kernel.handles_live(tcb).unwrap_or(0) as u64);
                Switch::Continue
            }
            SYS_HANDLE_CLOSE => {
                let result = self.close_handle(index, tcb, frame.arg0() as usize);
                frame.set_result(result);
                Switch::Continue
            }
            _ => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    /// 命中调度 tick 时保存现场；返回是否切换任务。
    fn send(&mut self, index: usize, tcb: TcbId, frame: &mut C, transfer: bool) -> Switch {
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
                    self.tasks[index].pending = Pending::Send;
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

    fn recv(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
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

    fn allocate(&mut self, index: usize, size: usize, align: usize) -> Option<u64> {
        let start = self.tasks[index].arena.allocate(size, align)?;
        self.platform.zero(start, size);
        Some(start)
    }

    fn notification_handle(&self, index: usize, number: usize) -> Option<Handle> {
        self.tasks[index]
            .notifications
            .get(number.wrapping_sub(1))
            .copied()
            .flatten()
    }

    fn notify(&self, index: usize, tcb: TcbId, frame: &mut C) -> u64 {
        let number = frame.arg0() as usize;
        let Some(handle) = self.notification_handle(index, number) else {
            return u64::MAX;
        };
        match self.kernel.signal(tcb, handle, frame.arg1()) {
            Ok(()) => 0,
            Err(_) => u64::MAX,
        }
    }

    fn wait_notify(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
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

    fn call(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
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

    fn reply(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> u64 {
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

    /// 把随消息收到的句柄加入任务额外句柄表，返回编号；没有句柄时为 0。
    fn close_handle(&mut self, index: usize, tcb: TcbId, endpoint: usize) -> u64 {
        let Some(handle) = self.handle_at(index, endpoint) else {
            return u64::MAX;
        };

        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                self.clear_handle(index, endpoint);
                0
            }
            Err(_) => u64::MAX,
        }
    }
}
