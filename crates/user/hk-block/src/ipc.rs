//! 块协议的服务端循环与远端块设备客户端。
//!
//! 协议编码在 [`hk_server_api::block`]；本模块把协议接到 [`AsyncBlockDevice`]，
//! 供 Block Server、virtio 驱动服务与 VFS 复用。
//!
//! 客户端使用多个共享帧槽位与递增标签，服务端按发送者与槽位分别映射帧，
//! 两边都只维护一个收发缓冲，数据面由帧承载。服务循环收下请求后立即提交给
//! 设备，完成回收时再回复，因此同一时刻可以有多条请求在设备队列中。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::{AsyncBlockDevice, BlockDevice, BlockError, SECTOR_SIZE};
use hk_proto::block::{
    BLOCK_ATTACH, BLOCK_DETACH, BLOCK_REQUEST, BLOCK_RESPONSE, BLOCK_SHUTDOWN, BUFFER_LEN,
    BUFFER_SLOTS, OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE, STATUS_BUSY, STATUS_ERROR,
    STATUS_INVALID, STATUS_OK, STATUS_RANGE, attach_length, attach_slot, attach_word,
    decode_request, decode_response, encode_request, encode_response,
};
use hk_server_api::{
    CapabilityKind, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, capability_phys, close_handle,
    handle_duplicate, map_attach, map_new, recv_data_handle, recv_data_handle_try, region_unmap,
    send, send_data, send_handle, service_endpoint, yield_now,
};

/// 直通帧（调用方已注册的帧）使用的槽位。
const DIRECT_SLOT: usize = 0;
/// 客户端自己管理的帧槽位起点。
const OWN_SLOT_START: usize = 1;
const CLIENT_BUFFER_SLOTS: usize = 8;
static CLIENT_MAPPING_VIRT: [AtomicU64; CLIENT_BUFFER_SLOTS] =
    [const { AtomicU64::new(0) }; CLIENT_BUFFER_SLOTS];
static CLIENT_MAPPING_HANDLE: [AtomicU64; CLIENT_BUFFER_SLOTS] =
    [const { AtomicU64::new(0) }; CLIENT_BUFFER_SLOTS];
static CLIENT_BUFFER_LEN: [AtomicU64; CLIENT_BUFFER_SLOTS] =
    [const { AtomicU64::new(0) }; CLIENT_BUFFER_SLOTS];

/// 登记一个 VFS 客户端缓冲区映射，供块客户端识别直写缓冲区。
pub fn register_client_mapping(virt: u64, handle: u64, len: usize) {
    for (slot, virt_slot) in CLIENT_MAPPING_VIRT.iter().enumerate() {
        let current = virt_slot.load(Ordering::Acquire);
        if current == 0 || current == virt {
            CLIENT_MAPPING_HANDLE[slot].store(handle, Ordering::Release);
            CLIENT_BUFFER_LEN[slot].store(len as u64, Ordering::Release);
            CLIENT_MAPPING_VIRT[slot].store(virt, Ordering::Release);
            return;
        }
    }
}

/// 注销一个 VFS 客户端缓冲区映射。
pub fn unregister_client_mapping(virt: u64) {
    for virt_slot in &CLIENT_MAPPING_VIRT {
        if virt_slot.load(Ordering::Acquire) == virt {
            virt_slot.store(0, Ordering::Release);
            return;
        }
    }
}

fn client_mapping_at(address: u64, len: usize) -> Option<(u64, u64, usize)> {
    for (slot, virt_slot) in CLIENT_MAPPING_VIRT.iter().enumerate() {
        let virt = virt_slot.load(Ordering::Acquire);
        let frame_len = CLIENT_BUFFER_LEN[slot].load(Ordering::Acquire) as usize;
        if virt != 0 && address == virt && len <= frame_len {
            let handle = CLIENT_MAPPING_HANDLE[slot].load(Ordering::Acquire);
            return Some((handle, virt, frame_len));
        }
    }
    None
}

/// 一个客户端帧槽位。
struct Slot {
    virt: u64,
    region: u64,
    capability: Option<u64>,
    attached: Option<u64>,
    inflight: bool,
}

/// 块协议客户端；缓冲位于堆上，避免大消息占用服务栈。
pub struct BlockClient {
    service: String,
    own_endpoint: u64,
    slots: Vec<Slot>,
    request: Vec<u8>,
    response: Vec<u8>,
    endpoint: u64,
    /// 已完成但尚未被对应请求取走的标签。
    deferred: Vec<(u32, u8, usize)>,
}

impl Drop for BlockClient {
    fn drop(&mut self) {
        if self.endpoint != 0 {
            let _ = send(self.endpoint, BLOCK_DETACH, u64::MAX);
        }
        for slot in &mut self.slots {
            if let Some(number) = slot.capability.take() {
                let _ = region_unmap(slot.region);
                let _ = close_handle(number);
            }
        }
    }
}

impl BlockClient {
    /// 构造客户端。
    pub fn new(service: &str, own_endpoint: u64) -> Self {
        let slots = (0..BUFFER_SLOTS as usize)
            .map(|_| Slot {
                virt: 0,
                region: 0,
                capability: None,
                attached: None,
                inflight: false,
            })
            .collect();
        Self {
            service: String::from(service),
            own_endpoint,
            slots,
            request: vec![0_u8; 64],
            response: vec![0_u8; 64 * 1024],
            endpoint: 0,
            deferred: Vec::new(),
        }
    }

    fn buffer(&mut self, slot: usize) -> &mut [u8] {
        // SAFETY: SYS_REGION_MAP 把 BUFFER_LEN 字节映射到该槽的虚拟地址。
        unsafe { core::slice::from_raw_parts_mut(self.slots[slot].virt as *mut u8, BUFFER_LEN) }
    }

    fn attach_mapping(
        &mut self,
        slot: usize,
        endpoint: u64,
        handle: u64,
        len: usize,
    ) -> Result<(), u8> {
        let duplicate = handle_duplicate(handle, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        if duplicate == u64::MAX {
            return Err(STATUS_ERROR);
        }
        if send_handle(
            endpoint,
            BLOCK_ATTACH,
            attach_word(slot as u32, len),
            duplicate,
        ) == u64::MAX
        {
            return Err(STATUS_ERROR);
        }
        Ok(())
    }

    fn ensure_slot(&mut self, slot: usize) -> Result<u64, u8> {
        let endpoint = service_endpoint(self.service.as_str());
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        if self.endpoint != endpoint {
            for entry in &mut self.slots {
                entry.attached = None;
            }
            self.endpoint = endpoint;
        }
        if self.slots[slot].capability.is_none() {
            let Some((number, region, virt)) = map_new(CapabilityKind::Anonymous, BUFFER_LEN)
            else {
                return Err(STATUS_ERROR);
            };
            self.slots[slot].capability = Some(number);
            self.slots[slot].region = region;
            self.slots[slot].virt = virt;
        }
        let number = self.slots[slot].capability.ok_or(STATUS_ERROR)?;
        if self.slots[slot].attached != Some(number) {
            self.attach_mapping(slot, endpoint, number, BUFFER_LEN)?;
            self.slots[slot].attached = Some(number);
        }
        Ok(endpoint)
    }

    fn ensure_external(&mut self, slot: usize, frame: (u64, u64, usize)) -> Result<u64, u8> {
        let endpoint = service_endpoint(self.service.as_str());
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        if self.endpoint != endpoint {
            for entry in &mut self.slots {
                entry.attached = None;
            }
            self.endpoint = endpoint;
        }
        if self.slots[slot].attached != Some(frame.0) {
            self.attach_mapping(slot, endpoint, frame.0, frame.2)?;
            self.slots[slot].attached = Some(frame.0);
        }
        Ok(endpoint)
    }

    fn submit_raw(&mut self, slot: usize, op: u8, block: u64, length: usize) -> Result<u32, u8> {
        if self.slots[slot].inflight {
            return Err(STATUS_BUSY);
        }
        let endpoint = self.endpoint;
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        let Some(encoded) = encode_request(
            &mut self.request,
            slot as u32,
            op,
            block,
            length as u32,
            &[],
        ) else {
            return Err(STATUS_INVALID);
        };
        if send_data(endpoint, BLOCK_REQUEST, &self.request[..encoded]) == u64::MAX {
            self.endpoint = 0;
            return Err(STATUS_ERROR);
        }
        self.slots[slot].inflight = true;
        Ok(slot as u32)
    }

    fn submit_own(&mut self, slot: usize, op: u8, block: u64, length: usize) -> Result<u32, u8> {
        self.ensure_slot(slot)?;
        self.submit_raw(slot, op, block, length)
    }

    /// 非阻塞回收一个完成；返回标签、状态与结果长度。
    pub fn poll(&mut self) -> Option<(u32, u8, usize)> {
        let endpoint = self.endpoint;
        if endpoint == 0 {
            return None;
        }
        let (label, _word, _sender, length, _handle) =
            recv_data_handle_try(self.own_endpoint, &mut self.response, endpoint);
        if label != BLOCK_RESPONSE {
            return None;
        }
        let (status, tag, body) = decode_response(&self.response[..length])?;
        if let Some(slot) = self.slots.get_mut(tag as usize) {
            slot.inflight = false;
        }
        Some((tag, status, body_u64(body)))
    }

    /// 等待指定标签的完成；其它标签的完成暂存，供各自的请求取走。
    pub fn poll_for(&mut self, tag: u32) -> Option<(u8, usize)> {
        if let Some(index) = self
            .deferred
            .iter()
            .position(|(pending, _, _)| *pending == tag)
        {
            let (_, status, written) = self.deferred.remove(index);
            return Some((status, written));
        }
        while let Some((completed, status, written)) = self.poll() {
            if completed == tag {
                return Some((status, written));
            }
            self.deferred.push((completed, status, written));
        }
        None
    }

    /// 阻塞回收一个完成。
    pub fn wait(&mut self) -> (u32, u8, usize) {
        hk_async::block_on(core::future::poll_fn(|context| {
            if let Some(completed) = self.poll() {
                core::task::Poll::Ready(completed)
            } else {
                context.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        }))
    }

    /// 读取块数据。
    pub fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        if buffer.len() > BUFFER_LEN {
            return Err(STATUS_INVALID);
        }
        if let Some(frame) = client_mapping_at(buffer.as_ptr() as u64, buffer.len()) {
            if self.ensure_external(DIRECT_SLOT, frame).is_ok()
                && let Ok(tag) = self.submit_raw(DIRECT_SLOT, OP_READ, block, buffer.len())
            {
                let (completed, status, written) = self.wait();
                if completed == tag {
                    if status != STATUS_OK {
                        return Err(status);
                    }
                    return Ok(written.min(buffer.len()));
                }
            }
            self.endpoint = 0;
            self.slots[DIRECT_SLOT].attached = None;
        }
        let tag = self.submit_own(OWN_SLOT_START, OP_READ, block, buffer.len())?;
        let (completed, status, written) = self.wait();
        if completed != tag {
            return Err(STATUS_INVALID);
        }
        if status != STATUS_OK {
            return Err(status);
        }
        let written = written.min(buffer.len());
        let virt = self.slots[OWN_SLOT_START].virt;
        // SAFETY: 该槽的帧在本客户端地址空间内有效，长度为 BUFFER_LEN。
        let source = unsafe { core::slice::from_raw_parts(virt as *const u8, written) };
        buffer[..written].copy_from_slice(source);
        Ok(written)
    }

    /// 写入块数据。
    pub fn write(&mut self, block: u64, data: &[u8]) -> Result<usize, u8> {
        if data.len() > BUFFER_LEN {
            return Err(STATUS_INVALID);
        }
        if let Some(frame) = client_mapping_at(data.as_ptr() as u64, data.len()) {
            if self.ensure_external(DIRECT_SLOT, frame).is_ok()
                && let Ok(tag) = self.submit_raw(DIRECT_SLOT, OP_WRITE, block, data.len())
            {
                let (completed, status, written) = self.wait();
                if completed == tag {
                    if status != STATUS_OK {
                        return Err(status);
                    }
                    return Ok(written.min(data.len()));
                }
            }
            self.endpoint = 0;
            self.slots[DIRECT_SLOT].attached = None;
        }
        self.ensure_slot(OWN_SLOT_START)?;
        {
            let frame = self.buffer(OWN_SLOT_START);
            frame[..data.len()].copy_from_slice(data);
        }
        let tag = self.submit_raw(OWN_SLOT_START, OP_WRITE, block, data.len())?;
        let (completed, status, written) = self.wait();
        if completed != tag {
            return Err(STATUS_INVALID);
        }
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(written.min(data.len()))
    }

    /// 刷写。
    pub fn flush(&mut self) -> Result<(), u8> {
        let tag = self.submit_own(OWN_SLOT_START, OP_FLUSH, 0, 0)?;
        let (completed, status, _) = self.wait();
        if completed != tag {
            return Err(STATUS_INVALID);
        }
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(())
    }

    /// 返回容量字节数。
    pub fn capacity(&mut self) -> Result<u64, u8> {
        let tag = self.submit_own(OWN_SLOT_START, OP_CAPACITY, 0, 0)?;
        let (completed, status, capacity) = self.wait();
        if completed != tag {
            return Err(STATUS_INVALID);
        }
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(capacity as u64)
    }

    fn idle_slot(&self) -> Option<usize> {
        (OWN_SLOT_START..self.slots.len()).find(|slot| !self.slots[*slot].inflight)
    }

    fn copy_out(&self, slot: usize, written: usize, buffer: &mut [u8], offset: usize) {
        let virt = self.slots[slot].virt;
        // SAFETY: 该槽的帧在本客户端地址空间内有效；`written` 不超过帧长度。
        let source = unsafe { core::slice::from_raw_parts(virt as *const u8, written) };
        buffer[offset..offset + written].copy_from_slice(source);
    }

    /// 读取一段连续区间，按帧槽位并行提交。
    pub fn read_bulk(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        if buffer.len() <= BUFFER_LEN {
            return self.read(block, buffer);
        }
        if !buffer.len().is_multiple_of(SECTOR_SIZE) {
            return Err(STATUS_INVALID);
        }
        let mut inflight: Vec<(u32, usize, usize)> = Vec::new();
        let mut submitted = 0usize;
        let mut failure: Option<u8> = None;
        while submitted < buffer.len() || !inflight.is_empty() {
            while failure.is_none()
                && inflight.len() < self.slots.len() - OWN_SLOT_START
                && submitted < buffer.len()
            {
                let length = (buffer.len() - submitted).min(BUFFER_LEN);
                let length = length / SECTOR_SIZE * SECTOR_SIZE;
                if length == 0 {
                    failure = Some(STATUS_INVALID);
                    break;
                }
                let Some(slot) = self.idle_slot() else {
                    break;
                };
                let sectors = block + (submitted / SECTOR_SIZE) as u64;
                match self.submit_own(slot, OP_READ, sectors, length) {
                    Ok(tag) => {
                        inflight.push((tag, submitted, length));
                        submitted += length;
                    }
                    Err(status) => {
                        failure = Some(status);
                        break;
                    }
                }
            }
            if inflight.is_empty() {
                break;
            }
            let (tag, status, written) = self.wait();
            let Some(index) = inflight.iter().position(|(entry, _, _)| *entry == tag) else {
                return Err(STATUS_INVALID);
            };
            let (_, offset, length) = inflight.remove(index);
            if status != STATUS_OK {
                failure = Some(status);
                continue;
            }
            let written = written.min(length);
            self.copy_out(tag as usize, written, buffer, offset);
            if written != length {
                failure = Some(STATUS_ERROR);
            }
        }
        match failure {
            Some(status) => Err(status),
            None => Ok(submitted),
        }
    }

    /// 写入一段连续区间，按帧槽位并行提交。
    pub fn write_bulk(&mut self, block: u64, data: &[u8]) -> Result<usize, u8> {
        if data.len() <= BUFFER_LEN {
            return self.write(block, data);
        }
        if !data.len().is_multiple_of(SECTOR_SIZE) {
            return Err(STATUS_INVALID);
        }
        let mut inflight: Vec<(u32, usize)> = Vec::new();
        let mut submitted = 0usize;
        let mut failure: Option<u8> = None;
        while submitted < data.len() || !inflight.is_empty() {
            while failure.is_none()
                && inflight.len() < self.slots.len() - OWN_SLOT_START
                && submitted < data.len()
            {
                let length = (data.len() - submitted).min(BUFFER_LEN);
                let length = length / SECTOR_SIZE * SECTOR_SIZE;
                if length == 0 {
                    failure = Some(STATUS_INVALID);
                    break;
                }
                let Some(slot) = self.idle_slot() else {
                    break;
                };
                if self.ensure_slot(slot).is_err() {
                    failure = Some(STATUS_ERROR);
                    break;
                }
                {
                    let frame = self.buffer(slot);
                    frame[..length].copy_from_slice(&data[submitted..submitted + length]);
                }
                let sectors = block + (submitted / SECTOR_SIZE) as u64;
                match self.submit_raw(slot, OP_WRITE, sectors, length) {
                    Ok(tag) => {
                        inflight.push((tag, length));
                        submitted += length;
                    }
                    Err(status) => {
                        failure = Some(status);
                        break;
                    }
                }
            }
            if inflight.is_empty() {
                break;
            }
            let (tag, status, written) = self.wait();
            let Some(index) = inflight.iter().position(|(entry, _)| *entry == tag) else {
                return Err(STATUS_INVALID);
            };
            let (_, length) = inflight.remove(index);
            if status != STATUS_OK || written.min(length) != length {
                failure = Some(if status == STATUS_OK {
                    STATUS_ERROR
                } else {
                    status
                });
            }
        }
        match failure {
            Some(status) => Err(status),
            None => Ok(submitted),
        }
    }
}

fn body_u64(body: &[u8]) -> usize {
    if body.len() < 8 {
        return 0;
    }
    u64::from_le_bytes(body[..8].try_into().unwrap_or([0; 8])) as usize
}

fn block_status(status: u8) -> BlockError {
    match status {
        STATUS_RANGE => BlockError::OutOfRange,
        STATUS_INVALID => BlockError::InvalidLength,
        STATUS_BUSY => BlockError::Busy,
        _ => BlockError::Io,
    }
}

/// 通过块协议访问远端块设备。
pub struct ClientDisk {
    client: RefCell<BlockClient>,
    capacity: u64,
}

impl ClientDisk {
    /// 连接块设备服务；连接失败时返回协议状态码。
    pub fn open(service: &str, own_endpoint: u64) -> Result<Self, u8> {
        let mut client = BlockClient::new(service, own_endpoint);
        let capacity = client.capacity()?;
        Ok(Self {
            client: RefCell::new(client),
            capacity,
        })
    }

    /// 返回设备容量，单位为字节。
    pub const fn capacity_bytes(&self) -> u64 {
        self.capacity
    }

    /// 异步读取一段块数据；未就绪时让出执行器。
    pub async fn read_async(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        if buffer.len() > BUFFER_LEN {
            return Err(BlockError::InvalidLength);
        }
        let mut submitted: Option<(usize, u32)> = None;
        core::future::poll_fn(|context| {
            let mut client = self.client.borrow_mut();
            let (slot, tag) = match submitted {
                Some(entry) => entry,
                None => {
                    let Some(slot) = client.idle_slot() else {
                        context.waker().wake_by_ref();
                        return core::task::Poll::Pending;
                    };
                    match client.submit_own(slot, OP_READ, block, buffer.len()) {
                        Ok(tag) => {
                            submitted = Some((slot, tag));
                            (slot, tag)
                        }
                        Err(status) => return core::task::Poll::Ready(Err(block_status(status))),
                    }
                }
            };
            if let Some((status, written)) = client.poll_for(tag) {
                if status != STATUS_OK {
                    return core::task::Poll::Ready(Err(block_status(status)));
                }
                let written = written.min(buffer.len());
                client.copy_out(slot, written, buffer, 0);
                return core::task::Poll::Ready(Ok(written));
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        })
        .await
    }

    /// 异步写入一段块数据；未就绪时让出执行器。
    pub async fn write_async(&self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        if data.len() > BUFFER_LEN {
            return Err(BlockError::InvalidLength);
        }
        let mut submitted: Option<(usize, u32)> = None;
        core::future::poll_fn(|context| {
            let mut client = self.client.borrow_mut();
            let (_slot, tag) = match submitted {
                Some(entry) => entry,
                None => {
                    let Some(slot) = client.idle_slot() else {
                        context.waker().wake_by_ref();
                        return core::task::Poll::Pending;
                    };
                    if client.ensure_slot(slot).is_err() {
                        return core::task::Poll::Ready(Err(BlockError::Io));
                    }
                    client.buffer(slot)[..data.len()].copy_from_slice(data);
                    match client.submit_raw(slot, OP_WRITE, block, data.len()) {
                        Ok(tag) => {
                            submitted = Some((slot, tag));
                            (slot, tag)
                        }
                        Err(status) => return core::task::Poll::Ready(Err(block_status(status))),
                    }
                }
            };
            if let Some((status, written)) = client.poll_for(tag) {
                if status != STATUS_OK {
                    return core::task::Poll::Ready(Err(block_status(status)));
                }
                return core::task::Poll::Ready(Ok(written.min(data.len())));
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        })
        .await
    }

    /// 异步读取一段连续区间；多槽位并行提交，未完成时让出执行器。
    pub async fn read_bulk_async(
        &self,
        block: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BlockError> {
        if buffer.len() <= BUFFER_LEN {
            return self.read_async(block, buffer).await;
        }
        if !buffer.len().is_multiple_of(SECTOR_SIZE) {
            return Err(BlockError::InvalidLength);
        }
        let mut inflight: Vec<(u32, usize, usize)> = Vec::new();
        let mut submitted = 0usize;
        let mut failure: Option<u8> = None;
        core::future::poll_fn(|context| {
            let mut client = self.client.borrow_mut();
            while let Some((tag, status, written)) = client.poll() {
                let Some(index) = inflight.iter().position(|(entry, _, _)| *entry == tag) else {
                    continue;
                };
                let (_, offset, length) = inflight.remove(index);
                if status != STATUS_OK {
                    failure = Some(status);
                    continue;
                }
                let written = written.min(length);
                client.copy_out(tag as usize, written, buffer, offset);
                if written != length {
                    failure = Some(STATUS_ERROR);
                }
            }
            while failure.is_none() && submitted < buffer.len() {
                let length = (buffer.len() - submitted).min(BUFFER_LEN);
                let length = length / SECTOR_SIZE * SECTOR_SIZE;
                if length == 0 {
                    failure = Some(STATUS_INVALID);
                    break;
                }
                let Some(slot) = client.idle_slot() else {
                    break;
                };
                let sectors = block + (submitted / SECTOR_SIZE) as u64;
                match client.submit_own(slot, OP_READ, sectors, length) {
                    Ok(tag) => {
                        inflight.push((tag, submitted, length));
                        submitted += length;
                    }
                    Err(status) => {
                        failure = Some(status);
                        break;
                    }
                }
            }
            if inflight.is_empty() {
                return core::task::Poll::Ready(match failure {
                    Some(status) => Err(block_status(status)),
                    None => Ok(submitted),
                });
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        })
        .await
    }

    /// 异步写入一段连续区间；多槽位并行提交，未完成时让出执行器。
    pub async fn write_bulk_async(&self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        if data.len() <= BUFFER_LEN {
            return self.write_async(block, data).await;
        }
        if !data.len().is_multiple_of(SECTOR_SIZE) {
            return Err(BlockError::InvalidLength);
        }
        let mut inflight: Vec<(u32, usize)> = Vec::new();
        let mut submitted = 0usize;
        let mut failure: Option<u8> = None;
        core::future::poll_fn(|context| {
            let mut client = self.client.borrow_mut();
            while let Some((tag, status, written)) = client.poll() {
                let Some(index) = inflight.iter().position(|(entry, _)| *entry == tag) else {
                    continue;
                };
                let (_, length) = inflight.remove(index);
                if status != STATUS_OK || written.min(length) != length {
                    failure = Some(if status == STATUS_OK {
                        STATUS_ERROR
                    } else {
                        status
                    });
                }
            }
            while failure.is_none() && submitted < data.len() {
                let length = (data.len() - submitted).min(BUFFER_LEN);
                let length = length / SECTOR_SIZE * SECTOR_SIZE;
                if length == 0 {
                    failure = Some(STATUS_INVALID);
                    break;
                }
                let Some(slot) = client.idle_slot() else {
                    break;
                };
                if client.ensure_slot(slot).is_err() {
                    failure = Some(STATUS_ERROR);
                    break;
                }
                {
                    let frame = client.buffer(slot);
                    frame[..length].copy_from_slice(&data[submitted..submitted + length]);
                }
                let sectors = block + (submitted / SECTOR_SIZE) as u64;
                match client.submit_raw(slot, OP_WRITE, sectors, length) {
                    Ok(tag) => {
                        inflight.push((tag, length));
                        submitted += length;
                    }
                    Err(status) => {
                        failure = Some(status);
                        break;
                    }
                }
            }
            if inflight.is_empty() {
                return core::task::Poll::Ready(match failure {
                    Some(status) => Err(block_status(status)),
                    None => Ok(submitted),
                });
            }
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        })
        .await
    }
}

impl BlockDevice for ClientDisk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.capacity / SECTOR_SIZE as u64
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        self.client
            .borrow_mut()
            .read(block, buffer)
            .map_err(block_status)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        self.client
            .borrow_mut()
            .write(block, data)
            .map_err(block_status)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        self.client.borrow_mut().flush().map_err(block_status)
    }

    fn read_blocks_bulk(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        self.client
            .borrow_mut()
            .read_bulk(block, buffer)
            .map_err(block_status)
    }

    fn write_blocks_bulk(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        self.client
            .borrow_mut()
            .write_bulk(block, data)
            .map_err(block_status)
    }
}

/// 服务端映射的一个帧。
struct ServerMapping {
    virt: u64,
    region: u64,
    len: usize,
    handle: u64,
    busy: bool,
}

/// 等待完成的一条请求。
struct Pending {
    sender: u64,
    slot: u32,
}

/// 在端点上服务块设备请求，直到任务退出或收到退出请求。
pub fn serve(endpoint: u64, disk: &mut dyn AsyncBlockDevice) {
    serve_with_hook(endpoint, disk, |_, _, _, _| {});
}

/// 服务块请求，并在共享帧映射时通知驱动登记 DMA 区域。
pub fn serve_with_hook<F: FnMut(u64, u64, usize, u64)>(
    endpoint: u64,
    disk: &mut dyn AsyncBlockDevice,
    hook: F,
) {
    serve_with_wait(endpoint, disk, hook, || {
        yield_now();
    });
}

/// 同 [`serve_with_hook`]，但等待在途完成时不轮询：`wait` 返回后读取设备中断状态。
pub fn serve_with_wait<F, W>(endpoint: u64, disk: &mut dyn AsyncBlockDevice, hook: F, wait: W)
where
    F: FnMut(u64, u64, usize, u64),
    W: FnMut(),
{
    serve_with_wait_and(endpoint, disk, hook, wait, |_, _, _, _, _| {});
}

/// 同 [`serve_with_wait`]，并把非块协议消息交给 `other` 处理。
///
/// `other` 收到 `(label, word, sender, 数据, 随消息收到的句柄)`；句柄由回调负责
/// 关闭。块协议标签由本函数处理，不会传入回调。
pub fn serve_with_wait_and<F, W, O>(
    endpoint: u64,
    disk: &mut dyn AsyncBlockDevice,
    mut hook: F,
    mut wait: W,
    mut other: O,
) where
    F: FnMut(u64, u64, usize, u64),
    W: FnMut(),
    O: FnMut(u64, u64, u64, &[u8], u64),
{
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut response = vec![0_u8; 64 * 1024];
    let mut frames: BTreeMap<(u64, u32), ServerMapping> = BTreeMap::new();
    let mut inflight: BTreeMap<u64, Pending> = BTreeMap::new();

    loop {
        while let Some((token, result)) = disk.poll_complete() {
            let Some(pending) = inflight.remove(&token) else {
                continue;
            };
            if let Some(frame) = frames.get_mut(&(pending.sender, pending.slot)) {
                frame.busy = false;
            }
            let (status, written) = match result {
                Ok(written) => (STATUS_OK, written),
                Err(error) => (status(error), 0),
            };
            reply(
                pending.sender,
                pending.slot,
                status,
                &(written as u64).to_le_bytes(),
                &mut response,
            );
        }

        let (label, word, sender, length, received) = if inflight.is_empty() {
            recv_data_handle(endpoint, &mut buffer, 0)
        } else {
            let message = recv_data_handle_try(endpoint, &mut buffer, 0);
            if message.0 == 0 {
                wait();
                disk.ack_interrupt();
                continue;
            }
            message
        };

        if !is_block_label(label) {
            other(
                label,
                word,
                sender,
                &buffer[..length.min(buffer.len())],
                received,
            );
            continue;
        }
        if label == BLOCK_SHUTDOWN {
            return;
        }
        if label == BLOCK_ATTACH {
            let slot = attach_slot(word);
            let length = attach_length(word);
            if received == 0 || length == 0 || length > BUFFER_LEN || slot >= BUFFER_SLOTS {
                if received != 0 {
                    let _ = close_handle(received);
                }
                continue;
            }
            let key = (sender, slot);
            let previous = frames.remove(&key);
            if let Some(old) = &previous {
                let _ = region_unmap(old.region);
                let _ = close_handle(old.handle);
            }
            let Some((region, virt)) = map_attach(received) else {
                let _ = close_handle(received);
                continue;
            };
            if let Some((phys, _)) = capability_phys(received) {
                hook(virt, phys, length, received);
            }
            frames.insert(
                key,
                ServerMapping {
                    virt,
                    region,
                    len: length,
                    handle: received,
                    busy: false,
                },
            );
            continue;
        }
        if label == BLOCK_DETACH {
            if word == u64::MAX {
                detach_sender(&mut frames, sender);
            } else if let Some(frame) = frames.remove(&(sender, word as u32)) {
                let _ = region_unmap(frame.region);
                let _ = close_handle(frame.handle);
            }
            continue;
        }
        if label != BLOCK_REQUEST {
            continue;
        }
        let Some(request) = decode_request(&buffer[..length.min(buffer.len())]) else {
            reply(sender, 0, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let Some(frame) = frames.get(&(sender, request.tag)) else {
            match request.op {
                OP_CAPACITY => {
                    reply(
                        sender,
                        request.tag,
                        STATUS_OK,
                        &disk.capacity_bytes().to_le_bytes(),
                        &mut response,
                    );
                }
                OP_FLUSH => {
                    let status = match disk.flush() {
                        Ok(()) => STATUS_OK,
                        Err(error) => status(error),
                    };
                    reply(sender, request.tag, status, &[], &mut response);
                }
                _ => reply(sender, request.tag, STATUS_INVALID, &[], &mut response),
            }
            continue;
        };
        if frame.busy {
            reply(sender, request.tag, STATUS_BUSY, &[], &mut response);
            continue;
        }
        let (virt, frame_len) = (frame.virt, frame.len);
        let result = match request.op {
            OP_READ | OP_WRITE => {
                let length = request.length as usize;
                if length == 0 || length > frame_len {
                    reply(sender, request.tag, STATUS_INVALID, &[], &mut response);
                    continue;
                }
                // SAFETY: 帧在客户端持有期间保持映射；服务端单线程，完成前不再访问。
                let data = unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, frame_len) };
                match request.op {
                    OP_READ => disk.submit_read(request.block, &mut data[..length]),
                    _ => disk.submit_write(request.block, &data[..length]),
                }
            }
            OP_FLUSH => match disk.flush() {
                Ok(()) => {
                    reply(
                        sender,
                        request.tag,
                        STATUS_OK,
                        &0u64.to_le_bytes(),
                        &mut response,
                    );
                    continue;
                }
                Err(error) => Err(error),
            },
            OP_CAPACITY => {
                reply(
                    sender,
                    request.tag,
                    STATUS_OK,
                    &disk.capacity_bytes().to_le_bytes(),
                    &mut response,
                );
                continue;
            }
            _ => {
                reply(sender, request.tag, STATUS_INVALID, &[], &mut response);
                continue;
            }
        };
        match result {
            Ok(token) => {
                if let Some(frame) = frames.get_mut(&(sender, request.tag)) {
                    frame.busy = true;
                }
                inflight.insert(
                    token,
                    Pending {
                        sender,
                        slot: request.tag,
                    },
                );
            }
            Err(error) => {
                reply(sender, request.tag, status(error), &[], &mut response);
            }
        }
    }
}

/// 判断标签是否属于块协议。
fn is_block_label(label: u64) -> bool {
    matches!(
        label,
        BLOCK_SHUTDOWN | BLOCK_ATTACH | BLOCK_DETACH | BLOCK_REQUEST
    )
}

fn detach_sender(frames: &mut BTreeMap<(u64, u32), ServerMapping>, sender: u64) {
    let keys: Vec<(u64, u32)> = frames
        .keys()
        .filter(|(owner, _)| *owner == sender)
        .copied()
        .collect();
    for key in keys {
        if let Some(frame) = frames.remove(&key) {
            let _ = region_unmap(frame.region);
            let _ = close_handle(frame.handle);
        }
    }
}

fn status(error: BlockError) -> u8 {
    match error {
        BlockError::OutOfRange => STATUS_RANGE,
        BlockError::InvalidLength => STATUS_INVALID,
        BlockError::Busy => STATUS_BUSY,
        BlockError::ReadOnly | BlockError::Io => STATUS_ERROR,
    }
}

fn reply(sender: u64, tag: u32, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, tag, status, data) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
    }
}
