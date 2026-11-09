//! 系统调用分派与 IPC 操作。

use super::*;

mod arena;
mod cadence;
mod capability;
mod capacity;
mod device;
mod dma;
mod framebuffer;
mod handle;
mod ipc;
mod region;
mod stats;
mod task;
mod thread;

const MAX_USER_DATA: usize = 64 * 1024;

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
            SYS_RECV_DATA_TRY => self.recv_data_try(index, tcb, frame),
            SYS_RECV_DATA_HANDLE => self.recv_data_handle(index, tcb, frame),
            SYS_RECV_DATA_HANDLE_TRY => self.recv_data_handle_try(index, tcb, frame),
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
            SYS_WAIT_NOTIFY_TRY => {
                let number = frame.arg0() as usize;
                let mask = frame.arg1();
                let result = match self.notification_handle(index, number) {
                    Some(handle) => self.kernel.try_wait(tcb, handle, mask).ok().flatten(),
                    None => None,
                };
                frame.set_result(result.unwrap_or(0));
                Switch::Continue
            }
            SYS_CALL => self.call(index, tcb, frame),
            SYS_REPLY => {
                let result = self.reply(index, tcb, frame, false);
                frame.set_result(result);
                Switch::Continue
            }
            SYS_REPLY_HANDLE => {
                let result = self.reply(index, tcb, frame, true);
                frame.set_result(result);
                Switch::Continue
            }
            SYS_FORK => self.fork_task(index, tcb, frame),
            SYS_THREAD_CREATE => self.thread_create(index, tcb, frame),
            SYS_THREAD_JOIN => self.thread_join(index, tcb, frame),
            SYS_THREAD_SET_TLS => self.thread_set_tls(index, frame),
            SYS_FUTEX_WAIT => self.futex_wait(index, tcb, frame),
            SYS_FUTEX_WAKE => self.futex_wake(index, frame),
            SYS_FAULT_SUBSCRIBE => self.fault_subscribe(index, frame),
            SYS_YIELD => {
                self.tasks[index].context = *frame;
                self.tasks[index].pending = Pending::None;
                Switch::Block
            }
            SYS_CADENCE_INFO => self.cadence_info(index, frame),
            SYS_METER_INFO => self.meter_info(index, frame),
            SYS_CADENCE_UPDATE => self.cadence_update(index, tcb, frame),
            SYS_CADENCE_SPLIT => self.cadence_split(index, tcb, frame),
            SYS_CADENCE_REVOKE => self.cadence_revoke(index, tcb, frame),
            SYS_TASK_ATTACH_CADENCE => self.task_attach_cadence(index, tcb, frame),
            SYS_CADENCE_SELF => {
                let result = match self.kernel.self_cadence_handle(tcb) {
                    Ok(handle) => self.install_extra_handle(index, handle),
                    Err(_) => u64::MAX,
                };
                frame.set_result(result);
                Switch::Continue
            }
            SYS_SLEEP => self.sleep(index, tcb, frame),
            SYS_EXIT => {
                let code = frame.arg0();
                self.release_grants(index);
                let _ = self.kernel.exit(tcb);
                self.tasks[index].exited = true;
                self.tasks[index].exit_code = code;
                self.wake_joiners(tcb);
                if self.tasks[index].leader == index {
                    self.exit_threads(index);
                }
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
            SYS_MSI_BIND => self.bind_msi(index, tcb, frame),
            SYS_SELF_NOTIFICATION => {
                frame.set_result(index as u64 + 1);
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
                let in_grant =
                    length != 0 && phys >= range.start().get() && end <= range.end().get();
                let in_frame = (0..self.tasks[index].capabilities.len()).any(|slot| {
                    self.capability_at(index, CAPABILITY_HANDLE_BASE + slot + 1)
                        .and_then(|(_, _, backing)| backing)
                        .is_some_and(|range| {
                            phys >= range.start().get() && end <= range.end().get()
                        })
                }) || (0..self.tasks[index].extra.len()).any(|slot| {
                    self.capability_at(index, EXTRA_HANDLE_BASE + slot + 1)
                        .and_then(|(_, _, backing)| backing)
                        .is_some_and(|range| {
                            phys >= range.start().get() && end <= range.end().get()
                        })
                });
                if !in_grant && !in_frame {
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
                let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
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
            SYS_CONSOLE_READ => {
                frame.set_result(match self.platform.console_read() {
                    Some(byte) => u64::from(byte),
                    None => u64::MAX,
                });
                Switch::Continue
            }
            SYS_CONSOLE_SUBSCRIBE => {
                self.console_subscriber = Some(index);
                frame.set_result(0);
                Switch::Continue
            }
            SYS_FRAMEBUFFER_INFO => {
                frame.set_result(self.framebuffer_info(index, frame));
                Switch::Continue
            }
            SYS_FRAMEBUFFER_MAP => {
                frame.set_result(self.framebuffer_map(index, frame));
                Switch::Continue
            }
            SYS_ARENA_INFO => self.arena_info(index, frame),
            SYS_CAPACITY_INFO => self.capacity_info(index, frame),
            SYS_CAPACITY_SPLIT => self.capacity_split(index, tcb, frame),
            SYS_CAPACITY_ALLOC => self.capacity_alloc(index, tcb, frame),
            SYS_CAPACITY_FREE => self.capacity_free(index, tcb, frame),
            SYS_CAPACITY_MERGE => self.capacity_merge(index, tcb, frame),
            SYS_FRAME_INFO => self.frame_info(index, frame),
            SYS_FRAME_PIN => self.frame_pin(index, tcb, frame),
            SYS_FRAME_SHARE => self.frame_share(index, tcb, frame),
            SYS_FRAME_RELEASE => self.frame_release(index, tcb, frame),
            SYS_FRAME_FREE => self.frame_free(index, tcb, frame),
            SYS_FRAME_MAP => self.frame_map(index, tcb, frame),
            SYS_MEMORY_STATS => self.memory_stats(frame),
            SYS_CAPABILITY_CREATE => self.capability_create(index, tcb, frame),
            SYS_CAPABILITY_CREATE_LAZY => self.capability_create_lazy(index, tcb, frame),
            SYS_REGION_MAP => self.region_map(index, tcb, frame),
            SYS_REGION_INFO => self.region_info(index, tcb, frame),
            SYS_REGION_UNMAP => self.region_unmap(index, tcb, frame),
            SYS_REGION_PROTECT => self.region_protect(index, tcb, frame),
            SYS_CAPABILITY_PHYS => self.capability_phys(index, frame),
            SYS_CAPABILITY_PIN => self.capability_pin(index, tcb, frame),
            SYS_CAPABILITY_SCANOUT => self.capability_scanout(index, tcb, frame),
            SYS_CAPABILITY_COMMIT => self.capability_commit(index, tcb, frame),
            SYS_CAPABILITY_EXPORT => self.capability_export(index, tcb, frame),
            SYS_CAPABILITY_BIND_PROVIDER => self.capability_bind_provider(index, tcb, frame),
            SYS_LEDGER_INFO => self.ledger_info(index, frame),
            SYS_HANDLE_DUPLICATE => self.duplicate_handle(index, tcb, frame),
            SYS_DMA_MAP_CAPABILITY => self.dma_map_capability(index, tcb, frame),
            SYS_DMA_MAP_FRAME => self.dma_map_frame(index, tcb, frame),
            SYS_BOOT_CMDLINE => {
                let pointer = frame.arg0();
                let capacity = frame.arg1() as usize;
                let result = match self.platform.boot_cmdline() {
                    Some(text) if text.len() <= capacity => {
                        if self.platform.write_user(pointer, text.as_bytes()) {
                            text.len() as u64
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
            SYS_KERNEL_STATS => {
                let threads = self.kernel.thread_count() as u64 & 0xFFFF;
                let objects = self.kernel.live_objects() as u64 & 0xFFFF;
                let runnable = self.kernel.runnable() as u64 & 0xFFFF;
                let traces = self.kernel.trace_len() as u64 & 0xFFFF;
                frame.set_result((threads << 48) | (objects << 32) | (runnable << 16) | traces);
                Switch::Continue
            }
            SYS_HANDLE_CLOSE => {
                let number = frame.arg0() as usize;
                let result = if number >= CAPABILITY_HANDLE_BASE {
                    self.close_handle(index, tcb, number)
                } else if number >= GRANT_HANDLE_BASE {
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
