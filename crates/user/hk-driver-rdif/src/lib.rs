//! `rdif-block` 控制器/队列到 `hk_block::AsyncBlockDevice` 的桥。
//!
//! 只运行一个硬件队列，请求使用驱动分配的 DMA 缓冲；控制器状态机在初始化
//! 阶段由 IRQ/寄存器重试推进，完成事件在 IRQ 后由 `poll_complete` 回收。

#![cfg_attr(target_os = "none", no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::time::Duration;

use dma_api::{CpuDmaBuffer, DeviceDma, DmaDirection};
use hk_block::{AsyncBlockDevice, BlockDevice, BlockError};
use rdif_block::{
    BlkError, BlockController, BlockControllerGroup, CompletionSink, ControllerEvent,
    ControllerState, ControllerUpdate, DeviceInfo, GroupControllerEvent, GroupControllerUpdate,
    GroupIrqEvent, GroupIrqSink, HardIrqHandler, HardwareQueue, OwnedRequest, OwnedRequestBatch,
    RequestFlags, RequestId, RequestOp, SharedHardIrqHandler, SubmissionSink,
};

/// 初始化期间推进寄存器状态所需的回调。
pub struct Waiter<'a> {
    /// 等待一次硬件中断通知。
    pub wait_irq: &'a mut dyn FnMut(),
}

struct Submission {
    id: Option<RequestId>,
}

impl SubmissionSink for Submission {
    fn accepted(&mut self, id: RequestId) {
        self.id = Some(id);
    }
}

struct Completions(Vec<rdif_block::CompletedRequest>);

impl CompletionSink for Completions {
    fn complete(&mut self, request: rdif_block::CompletedRequest) {
        self.0.push(request);
    }
}

struct Pending {
    token: u64,
    pointer: Option<NonNull<u8>>,
    len: usize,
}

struct Inner {
    controller: Box<dyn BlockController>,
    group: Option<Box<dyn BlockControllerGroup>>,
    queue: Box<dyn HardwareQueue>,
    handlers: Vec<Box<dyn HardIrqHandler>>,
    group_handlers: Vec<Box<dyn SharedHardIrqHandler>>,
    pending: BTreeMap<usize, Pending>,
    token: u64,
}

/// 一个可用的 rdif 块设备。
pub struct RdifDisk {
    inner: spin::Mutex<Inner>,
    dma: DeviceDma,
    info: DeviceInfo,
}

impl RdifDisk {
    /// 从一个已启动的单控制器创建块设备。
    pub fn open_controller(
        mut controller: Box<dyn BlockController>,
        dma: DeviceDma,
        waiter: &mut Waiter<'_>,
    ) -> Result<Self, BlkError> {
        let mut handlers = Vec::new();
        let mut queue = None;
        let mut info = None;
        let mut update = controller.advance(ControllerEvent::Start { target_queues: 1 })?;
        loop {
            collect_controller(&mut update, &mut handlers, &mut queue, &mut info);
            match update.controller_state() {
                ControllerState::RegisterPending { retry_after } => {
                    delay(retry_after);
                    update = controller.advance(ControllerEvent::RegisterRetry)?;
                }
                ControllerState::WaitingForIrq => {
                    (waiter.wait_irq)();
                    let controls = acknowledge(&mut handlers);
                    for control in controls {
                        update = controller.advance(ControllerEvent::Irq(control))?;
                        collect_controller(&mut update, &mut handlers, &mut queue, &mut info);
                    }
                }
                ControllerState::Ready => break,
                ControllerState::Shutdown => return Err(BlkError::Io),
            }
        }
        let queue = queue.ok_or(BlkError::Io)?;
        let info = info.ok_or(BlkError::Io)?;
        Ok(Self {
            inner: spin::Mutex::new(Inner {
                controller,
                group: None,
                queue,
                handlers,
                group_handlers: Vec::new(),
                pending: BTreeMap::new(),
                token: 1,
            }),
            dma,
            info,
        })
    }

    /// 从 AHCI 等控制器组创建块设备；使用第一个成员。
    pub fn open_group(
        mut group: Box<dyn BlockControllerGroup>,
        dma: DeviceDma,
        waiter: &mut Waiter<'_>,
    ) -> Result<Self, BlkError> {
        let mut group_handlers = Vec::new();
        let mut update = group.advance(GroupControllerEvent::Start)?;
        let member = loop {
            collect_group(&mut update, &mut group_handlers);
            let members = update.take_members();
            if !members.is_empty() {
                break members
                    .into_iter()
                    .next()
                    .ok_or(BlkError::Io)?
                    .into_controller();
            }
            match update.controller_state() {
                ControllerState::RegisterPending { retry_after } => {
                    delay(retry_after);
                    update = group.advance(GroupControllerEvent::RegisterRetry)?;
                }
                ControllerState::WaitingForIrq => {
                    (waiter.wait_irq)();
                    for handler in &mut group_handlers {
                        let _ = handler.ack(&mut GroupSink);
                    }
                    update = group.advance(GroupControllerEvent::Irq(
                        rdif_block::ControlEvent::new(0, 0),
                    ))?;
                }
                ControllerState::Ready => {
                    break update
                        .take_members()
                        .into_iter()
                        .next()
                        .ok_or(BlkError::Io)?
                        .into_controller();
                }
                ControllerState::Shutdown => return Err(BlkError::Io),
            }
        };
        let disk = Self::open_controller(member, dma, waiter)?;
        {
            let mut inner = disk.inner.lock();
            inner.group = Some(group);
            inner.group_handlers = group_handlers;
        }
        Ok(disk)
    }

    /// 块大小。
    pub fn block_size(&self) -> usize {
        self.info.logical_block_size.max(1)
    }

    /// 块数量。
    pub fn capacity_blocks(&self) -> u64 {
        self.info.num_blocks
    }

    fn submit(&self, block: u64, buffer: &[u8], write: bool) -> Result<u64, BlockError> {
        let block_size = self.block_size();
        if buffer.is_empty() || !buffer.len().is_multiple_of(block_size) {
            return Err(BlockError::InvalidLength);
        }
        let blocks = buffer.len() / block_size;
        if blocks > u32::MAX as usize
            || block
                .checked_add(blocks as u64)
                .is_none_or(|end| end > self.info.num_blocks)
        {
            return Err(BlockError::OutOfRange);
        }
        let direction = if write {
            DmaDirection::ToDevice
        } else {
            DmaDirection::FromDevice
        };
        let length = NonZeroUsize::new(buffer.len()).ok_or(BlockError::InvalidLength)?;
        let mut dma = CpuDmaBuffer::new_zero(&self.dma, length, block_size, direction)
            .map_err(|_| BlockError::Io)?;
        if write {
            dma.copy_from_slice_cpu(buffer);
        }
        let prepared = dma.prepare_for_device();
        let request = OwnedRequest {
            op: if write {
                RequestOp::Write
            } else {
                RequestOp::Read
            },
            lba: block,
            block_count: blocks as u32,
            data: Some(prepared),
            flags: RequestFlags::NONE,
        };
        let mut batch = OwnedRequestBatch::with_capacity(1);
        batch.push_back(request);
        let mut sink = Submission { id: None };
        let mut inner = self.inner.lock();
        let result = inner.queue.submit_batch_owned(&mut batch, &mut sink);
        if result.accepted() != 1 {
            if result.accepted() > 0 {
                let _ = inner.queue.commit_submissions();
            }
            return Err(BlockError::Busy);
        }
        if inner.queue.commit_submissions().is_err() {
            return Err(BlockError::Io);
        }
        let id = sink.id.ok_or(BlockError::Io)?.into();
        let token = inner.token;
        inner.token = inner.token.wrapping_add(1);
        inner.pending.insert(
            id,
            Pending {
                token,
                pointer: (!write)
                    .then(|| NonNull::new(buffer.as_ptr() as *mut u8))
                    .flatten(),
                len: buffer.len(),
            },
        );
        Ok(token)
    }

    fn poll(&self) -> Option<(u64, Result<usize, BlockError>)> {
        let mut inner = self.inner.lock();
        let mut completions = Completions(Vec::new());
        if inner.queue.drain_completions(&mut completions).is_err() {
            return None;
        }
        for completion in completions.0 {
            let id: usize = completion.id.into();
            let Some(pending) = inner.pending.remove(&id) else {
                continue;
            };
            let result = match (completion.result, pending.pointer) {
                (Err(error), _) => Err(block_error(error)),
                (Ok(()), Some(pointer)) => match completion.data {
                    Some(data) => {
                        // SAFETY: pending 指针来自 submit 时调用方提供的缓冲，生命周期覆盖完成。
                        let buffer = unsafe {
                            core::slice::from_raw_parts_mut(pointer.as_ptr(), pending.len)
                        };
                        data.copy_to_slice_cpu(buffer);
                        Ok(pending.len)
                    }
                    None => Err(BlockError::Io),
                },
                (Ok(()), None) => Ok(pending.len),
            };
            return Some((pending.token, result));
        }
        None
    }

    fn wait(&self, token: u64) -> Result<usize, BlockError> {
        for _ in 0..10_000_000 {
            if let Some((done, result)) = self.poll()
                && done == token
            {
                return result;
            }
            core::hint::spin_loop();
        }
        Err(BlockError::Io)
    }
}

impl BlockDevice for RdifDisk {
    fn block_size(&self) -> usize {
        self.info.logical_block_size.max(1)
    }

    fn capacity_blocks(&self) -> u64 {
        self.info.num_blocks
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        let token = self.submit(block, buffer, false)?;
        self.wait(token)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        let token = self.submit(block, data, true)?;
        self.wait(token)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

impl AsyncBlockDevice for RdifDisk {
    fn submit_read(&mut self, block: u64, buffer: &mut [u8]) -> Result<u64, BlockError> {
        self.submit(block, buffer, false)
    }

    fn submit_write(&mut self, block: u64, data: &[u8]) -> Result<u64, BlockError> {
        self.submit(block, data, true)
    }

    fn poll_complete(&mut self) -> Option<(u64, Result<usize, BlockError>)> {
        self.poll()
    }

    fn ack_interrupt(&mut self) {
        let mut inner = self.inner.lock();
        for handler in &mut inner.group_handlers {
            let _ = handler.ack(&mut GroupSink);
        }
        let controls = acknowledge(&mut inner.handlers);
        for control in controls {
            if let Ok(mut update) = inner.controller.advance(ControllerEvent::Irq(control)) {
                let mut handlers = Vec::new();
                let mut queue = None;
                let mut info = None;
                collect_controller(&mut update, &mut handlers, &mut queue, &mut info);
                inner.handlers.extend(handlers);
            }
        }
    }
}

fn block_error(error: BlkError) -> BlockError {
    match error {
        BlkError::InvalidBlockIndex(_) | BlkError::InvalidRequest => BlockError::OutOfRange,
        BlkError::NotSupported => BlockError::InvalidLength,
        BlkError::Io | BlkError::TimedOut => BlockError::Io,
        _ => BlockError::Io,
    }
}

fn delay(duration: Duration) {
    let spins = duration.as_micros().clamp(1, 10_000) as usize;
    for _ in 0..spins {
        core::hint::spin_loop();
    }
}

fn acknowledge(handlers: &mut [Box<dyn HardIrqHandler>]) -> Vec<rdif_block::ControlEvent> {
    let mut controls = Vec::new();
    for handler in handlers {
        let ack = handler.ack();
        if !ack.is_spurious() {
            controls.push(ack.control_event());
        }
    }
    controls
}

fn collect_controller(
    update: &mut ControllerUpdate,
    handlers: &mut Vec<Box<dyn HardIrqHandler>>,
    queue: &mut Option<Box<dyn HardwareQueue>>,
    info: &mut Option<DeviceInfo>,
) {
    for endpoint in update.take_irq_endpoints() {
        handlers.push(endpoint.into_handler());
    }
    if queue.is_none()
        && let Some(queues) = update.take_queues().into_iter().next()
    {
        *queue = Some(queues);
    }
    if info.is_none() {
        *info = update.take_device_info();
    }
}

fn collect_group(
    update: &mut GroupControllerUpdate,
    handlers: &mut Vec<Box<dyn SharedHardIrqHandler>>,
) {
    for endpoint in update.take_irq_endpoints() {
        handlers.push(endpoint.into_handler());
    }
}

struct GroupSink;

impl GroupIrqSink for GroupSink {
    fn publish(&mut self, _event: GroupIrqEvent) {}
}
