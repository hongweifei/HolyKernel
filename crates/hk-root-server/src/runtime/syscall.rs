//! 系统调用分派与 IPC 操作。

use super::*;

const MAX_USER_DATA: usize = 4096;

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
            SYS_SEND_DATA => self.send_data(index, tcb, frame),
            SYS_RECV_DATA => self.recv_data(index, tcb, frame),
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
                self.release_grants(index);
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
            SYS_DEVICE_GRANT_CREATE => self.create_grant(index, tcb, frame),
            SYS_DEVICE_MAP => self.map_grant(index, tcb, frame),
            SYS_IRQ_BIND => self.bind_irq(index, tcb, frame),
            SYS_SELF_NOTIFICATION => {
                frame.set_result(index as u64 + 1);
                Switch::Continue
            }
            SYS_DMA_ALLOC => {
                let grant_number = frame.arg0() as usize;
                let length = frame.arg1() as usize;
                let virt = frame.arg2();
                let Some(handle) = self.grant_at(index, grant_number) else {
                    frame.set_result(0);
                    return Switch::Continue;
                };
                let Ok(grant) = self.kernel.device_grant(tcb, handle) else {
                    frame.set_result(0);
                    return Switch::Continue;
                };
                let Some(range) = grant.dma else {
                    frame.set_result(0);
                    return Switch::Continue;
                };
                if length == 0
                    || length as u64 > range.len()
                    || !virt.is_multiple_of(hk_hal::PAGE_SIZE as u64)
                    || !self.platform.map_dma(
                        index,
                        virt,
                        range.start().get(),
                        range.len() as usize,
                    )
                {
                    frame.set_result(0);
                } else {
                    frame.set_result(range.start().get());
                }
                Switch::Continue
            }
            SYS_PCI_WRITE => {
                let address = frame.arg0();
                let bus = (address >> 8) as u8;
                let device = ((address >> 3) & 0x1F) as u8;
                let function = (address & 0x07) as u8;
                self.platform.pci_write(
                    bus,
                    device,
                    function,
                    frame.arg1() as u8,
                    frame.arg2() as u32,
                );
                frame.set_result(0);
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
                let number = frame.arg0() as usize;
                let result = if number >= GRANT_HANDLE_BASE {
                    self.close_grant(index, tcb, number)
                } else {
                    self.close_handle(index, tcb, number)
                };
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
    /// 发送带字节载荷的消息。
    fn send_data(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
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
                    self.tasks[index].pending = Pending::Send;
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
    fn recv_data(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
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

        match self.kernel.recv(tcb, handle) {
            Ok(Some(received)) => self.deliver_data(index, frame, &received, buffer, capacity),
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::RecvData {
                    handle,
                    buffer,
                    capacity,
                };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
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
    fn create_grant(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        if !self.tasks[index].device_authority {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let length = frame.arg1() as usize;
        if length == 0 || length > hk_hal::DEVICE_GRANT_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
        if !self.platform.read_user(frame.arg0(), &mut bytes[..length]) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(mut grant) = DeviceGrant::decode(&bytes[..length]) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        grant.dma = self.platform.dma_window(DMA_WINDOW_BYTES);
        match self.kernel.create_device_grant(
            tcb,
            grant,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::TRANSFER,
        ) {
            Ok(handle) => {
                frame.set_result(self.install_grant(index, handle));
                Switch::Continue
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }
    fn map_grant(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let resource = frame.arg1() as usize;
        let virt = frame.arg2();
        let Some(handle) = self.grant_at(index, grant_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(grant) = self.kernel.device_grant(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(range) = grant.mmio.get(resource).copied().flatten() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !virt.is_multiple_of(hk_hal::PAGE_SIZE as u64)
            || !self
                .platform
                .map_mmio(index, virt, range.base, range.len as usize)
        {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(0);
        }
        Switch::Continue
    }
    fn bind_irq(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let notification_number = frame.arg1() as usize;
        let bits = frame.arg2();
        let Some(grant) = self.grant_at(index, grant_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(notification) = self.notification_handle(index, notification_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(descriptor) = self.kernel.device_grant(tcb, grant) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(irq) = descriptor.irq else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self.platform.route_irq(irq) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        match self.kernel.bind_irq(tcb, grant, notification, bits) {
            Ok(_) => {
                let count = &mut self.irq_routes[irq as usize];
                *count = count.saturating_add(1);
                self.tasks[index].bound_irqs.push((grant, irq));
                frame.set_result(0);
            }
            Err(_) => {
                if self.irq_routes[irq as usize] == 0 {
                    self.platform.unroute_irq(irq);
                }
                frame.set_result(u64::MAX);
            }
        }
        Switch::Continue
    }
    fn close_grant(&mut self, index: usize, tcb: TcbId, number: usize) -> u64 {
        let Some(slot) = number.checked_sub(GRANT_HANDLE_BASE + 1) else {
            return u64::MAX;
        };
        let Some(handle) = self.tasks[index].grants.get(slot).copied().flatten() else {
            return u64::MAX;
        };
        self.release_bindings(index, handle);
        if let Ok(grant) = self.kernel.device_grant(tcb, handle)
            && let Some(range) = grant.dma
        {
            self.platform.dma_free(range);
        }
        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                self.tasks[index].grants[slot] = None;
                0
            }
            Err(_) => u64::MAX,
        }
    }
}
