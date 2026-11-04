//! 系统调用分派与 IPC 操作。

use super::*;

mod device;
mod ipc;
mod task;

const MAX_USER_DATA: usize = 8192;

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
            SYS_RECV_DATA_FROM => self.recv_data(index, tcb, frame),
            SYS_SERVICE_ENDPOINT => {
                let length = frame.arg1() as usize;
                let mut name = [0_u8; 64];
                if length == 0
                    || length > name.len()
                    || !self.platform.read_user(frame.arg0(), &mut name[..length])
                {
                    frame.set_result(0);
                } else if let Ok(name) = core::str::from_utf8(&name[..length]) {
                    let endpoint = self
                        .tasks
                        .iter()
                        .position(|task| !task.exited && task.name == name)
                        .map_or(0, |slot| slot as u64 + 1);
                    frame.set_result(endpoint);
                } else {
                    frame.set_result(0);
                }
                Switch::Continue
            }
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
                let page = hk_hal::PAGE_SIZE as u64;
                let aligned = (length as u64).div_ceil(page) * page;
                let used = self.tasks[index]
                    .dma_used
                    .iter()
                    .find(|(bound, _)| *bound == handle)
                    .map_or(0, |(_, used)| *used);
                if length == 0
                    || aligned > range.len().saturating_sub(used)
                    || !virt.is_multiple_of(page)
                    || !self.platform.map_dma(
                        index,
                        virt,
                        range.start().get() + used,
                        aligned as usize,
                    )
                {
                    frame.set_result(0);
                    return Switch::Continue;
                }
                let phys = range.start().get() + used;
                let iova = hk_hal::Iova::new(phys);
                let domain = hk_hal::IommuDomain::new(
                    (u32::from(grant.address.bus) << 8)
                        | (u32::from(grant.address.device) << 3)
                        | u32::from(grant.address.function),
                );
                if let Some(iommu) = self.platform.iommu()
                    && iommu
                        .map(
                            domain,
                            iova,
                            hk_hal::PhysRange::new(hk_hal::PhysAddr::new(phys), aligned),
                            hk_hal::IommuFlags::READ | hk_hal::IommuFlags::WRITE,
                        )
                        .is_err()
                {
                    frame.set_result(0);
                    return Switch::Continue;
                }
                if let Some((_, used)) = self.tasks[index]
                    .dma_used
                    .iter_mut()
                    .find(|(bound, _)| *bound == handle)
                {
                    *used += aligned;
                } else {
                    self.tasks[index].dma_used.push((handle, aligned));
                }
                if self.platform.iommu().is_some() {
                    self.tasks[index]
                        .dma_mappings
                        .push((handle, domain, iova, aligned as usize));
                }
                frame.set_result(phys);
                Switch::Continue
            }
            SYS_DMA_SYNC => {
                let grant_number = frame.arg0() as usize;
                let phys = frame.arg1();
                let length = frame.arg2() as usize;
                let to_device = frame.arg3() != 0;
                let Some(handle) = self.grant_at(index, grant_number) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                let Ok(grant) = self.kernel.device_grant(tcb, handle) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                let Some(range) = grant.dma else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                let Some(end) = phys.checked_add(length as u64) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                if length == 0 || phys < range.start().get() || end > range.end().get() {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
                if let Some(cache) = self.platform.cache_ops() {
                    let range = hk_hal::PhysRange::new(hk_hal::PhysAddr::new(phys), length as u64);
                    if to_device {
                        cache.clean(range);
                    } else {
                        cache.invalidate(range);
                    }
                    cache.barrier();
                }
                frame.set_result(0);
                Switch::Continue
            }
            SYS_DEVICE_INFO => {
                let number = frame.arg0() as usize;
                let capacity = frame.arg1() as usize;
                let output = frame.arg2();
                let Some(handle) = self.grant_at(index, number) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                let Ok(grant) = self.kernel.device_grant(tcb, handle) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
                let Some(length) = grant.encode(&mut bytes) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                if length > capacity || !self.platform.write_user(output, &bytes[..length]) {
                    frame.set_result(u64::MAX);
                } else {
                    frame.set_result(length as u64);
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
            SYS_CIVIL_TIME => {
                let pointer = frame.arg0();
                let capacity = frame.arg1() as usize;
                let result = match self.platform.civil_time() {
                    Some(time) if capacity >= CivilTime::ENCODED_LEN => {
                        let mut bytes = [0_u8; CivilTime::ENCODED_LEN];
                        time.encode(&mut bytes);
                        if self.platform.write_user(pointer, &bytes) {
                            CivilTime::ENCODED_LEN as u64
                        } else {
                            u64::MAX
                        }
                    }
                    Some(_) => u64::MAX,
                    None => 0,
                };
                frame.set_result(result);
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
}
