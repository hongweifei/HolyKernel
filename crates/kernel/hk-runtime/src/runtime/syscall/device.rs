use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn create_grant(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let Some(factory) = self.process(index).device_factory else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg1() as usize;
        if length == 0 || length > hk_device::DEVICE_GRANT_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
        if !self.platform.read_user(frame.arg0(), &mut bytes[..length]) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(mut grant) = DeviceGrant::decode(&bytes[..length]) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(charge) = self
            .process_mut(index)
            .ledger
            .charge(DMA_WINDOW_BYTES as u64)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(range) = self.platform.dma_window(DMA_WINDOW_BYTES) else {
            let _ = self.process_mut(index).ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        grant.dma = Some(range);
        match self.kernel.create_device_grant_authorized(
            tcb,
            factory,
            grant,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::TRANSFER,
        ) {
            Ok(handle) => {
                frame.set_result(self.install_grant(index, handle, Some(charge)));
                Switch::Continue
            }
            Err(_) => {
                self.platform.dma_free(range);
                let _ = self.process_mut(index).ledger.release(charge);
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    pub(crate) fn map_grant(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let resource = frame.arg1() as usize;
        let out = frame.arg2();
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
        let page = hk_hal::PAGE_SIZE as u64;
        let aligned = range.len.div_ceil(page) * page;
        if out == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(virt) = self
            .process_mut(index)
            .user_window
            .allocate(aligned as usize)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self.platform.write_user(out, &virt.to_le_bytes()) {
            self.process_mut(index).user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if !self
            .platform
            .map_mmio(index, virt, range.base, aligned as usize)
        {
            self.process_mut(index).user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if !self.install_device_mapping(index, tcb, range.base, aligned as usize, virt) {
            let _ = self.platform.unmap_user(index, virt, aligned as usize);
            self.process_mut(index).user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn bind_irq(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        self.bind_irq_route(index, tcb, frame, false)
    }

    /// 订阅 ACPI SCI/GPE 事件；事件到达时投递通知。
    pub(crate) fn acpi_listen(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let notification_number = frame.arg0() as usize;
        let Some(notification) = self.notification_handle(index, notification_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(notification_id) = self.kernel.handle_id(tcb, notification) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let line = hk_hal::irq::ACPI_SCI_IRQ;
        let count = &mut self.irq_routes[line as usize];
        *count = count.saturating_add(1);
        self.tasks[index].bound_irqs.push((
            hk_handle::Handle::new(u32::MAX, u32::MAX),
            line,
            hk_hal::NotificationId::new(notification_id.get()),
            1,
        ));
        frame.set_result(0);
        Switch::Continue
    }

    /// 绑定 MSI/MSI-X 向量：只记录通知目标，不建立 IOAPIC 路由。
    pub(crate) fn bind_msi(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        self.bind_irq_route(index, tcb, frame, true)
    }

    fn bind_irq_route(&mut self, index: usize, tcb: TcbId, frame: &mut C, direct: bool) -> Switch {
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
        let Some(controller) = self.platform.interrupt_controller() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(notification_id) = self.kernel.handle_id(tcb, notification) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let irq = hk_hal::Irq::new(u32::from(irq));
        let target = hk_hal::NotificationId::new(notification_id.get());
        let bound = if direct {
            controller.bind_direct(irq, target)
        } else {
            controller.bind(irq, target)
        };
        if bound.is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let count = &mut self.irq_routes[irq.get() as usize];
        *count = count.saturating_add(1);
        self.tasks[index].bound_irqs.push((
            grant,
            irq.get() as u8,
            hk_hal::NotificationId::new(notification_id.get()),
            bits,
        ));
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn close_grant(&mut self, index: usize, tcb: TcbId, number: usize) -> u64 {
        let Some(slot) = number.checked_sub(GRANT_HANDLE_BASE + 1) else {
            return u64::MAX;
        };
        let Some(handle) = self.tasks[index]
            .grants
            .get(slot)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.handle)
        else {
            return u64::MAX;
        };
        self.release_bindings(index, handle);
        self.release_dma_mappings(index, handle);
        if let Ok(grant) = self.kernel.device_grant(tcb, handle)
            && let Some(range) = grant.dma
        {
            self.platform.dma_free(range);
        }
        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                let entry = self.tasks[index].grants[slot].take();
                if let Some(charge) = entry.and_then(|entry| entry.charge) {
                    let _ = self.process_mut(index).ledger.release(charge);
                }
                0
            }
            Err(_) => u64::MAX,
        }
    }
}
