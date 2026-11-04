//! 块协议的服务端循环与远端块设备客户端。
//!
//! 协议编码在 [`hk_server_api::block`]；本模块把协议接到 [`BlockDevice`]，
//! 供 Block Server、virtio 驱动服务与 VFS 复用。

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::{BlockDevice, BlockError, SECTOR_SIZE};
use hk_server_api::block::{
    BLOCK_ATTACH, BLOCK_REQUEST, BLOCK_RESPONSE, BLOCK_SHUTDOWN, FRAME_LEN, FRAME_VIRT,
    OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE, RESPONSE_HEADER, Request, STATUS_ERROR,
    STATUS_INVALID, STATUS_OK, STATUS_RANGE, decode_request, decode_response, encode_request,
    encode_response,
};
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, frame_alloc, handle_duplicate, recv_data,
    recv_data_from, send_data, send_handle, service_endpoint,
};

/// 块协议客户端；缓冲位于堆上，避免大消息占用服务栈。
pub struct BlockClient {
    service: &'static str,
    own_endpoint: u64,
    request: Vec<u8>,
    response: Vec<u8>,
    frame: Option<u64>,
    attached: u64,
}

impl BlockClient {
    /// 构造客户端。
    pub fn new(service: &'static str, own_endpoint: u64) -> Self {
        Self {
            service,
            own_endpoint,
            request: vec![0_u8; 64 * 1024],
            response: vec![0_u8; 64 * 1024],
            frame: None,
            attached: 0,
        }
    }

    fn ensure_attach(&mut self) -> Result<u64, u8> {
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        if self.attached == endpoint && self.frame.is_some() {
            return Ok(endpoint);
        }
        if self.frame.is_none() {
            let number = frame_alloc(FRAME_LEN, FRAME_VIRT);
            if number == 0 || number == u64::MAX {
                return Err(STATUS_ERROR);
            }
            self.frame = Some(number);
        }
        let number = self.frame.ok_or(STATUS_ERROR)?;
        let duplicate = handle_duplicate(number, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        if duplicate == u64::MAX {
            return Err(STATUS_ERROR);
        }
        if send_handle(endpoint, BLOCK_ATTACH, FRAME_LEN as u64, duplicate) == u64::MAX {
            self.frame = None;
            return Err(STATUS_ERROR);
        }
        self.attached = endpoint;
        Ok(endpoint)
    }

    fn frame(&mut self) -> &mut [u8] {
        // SAFETY: SYS_FRAME_ALLOC 把 FRAME_LEN 字节映射到 FRAME_VIRT。
        unsafe { core::slice::from_raw_parts_mut(FRAME_VIRT as *mut u8, FRAME_LEN) }
    }

    fn request(&mut self, op: u8, block: u64, length: u32) -> Result<(), u8> {
        let endpoint = self.ensure_attach()?;
        let Some(request_length) = encode_request(&mut self.request, op, block, length, &[]) else {
            return Err(STATUS_INVALID);
        };
        if send_data(endpoint, BLOCK_REQUEST, &self.request[..request_length]) == u64::MAX {
            self.attached = 0;
            return Err(STATUS_ERROR);
        }
        let (label, _word, _sender, response_length) =
            recv_data_from(self.own_endpoint, endpoint, &mut self.response);
        if label != BLOCK_RESPONSE {
            self.attached = 0;
            return Err(STATUS_ERROR);
        }
        let Some((status, _)) = decode_response(&self.response[..response_length]) else {
            return Err(STATUS_INVALID);
        };
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(())
    }

    fn written(&self) -> Result<usize, u8> {
        let bytes = &self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8];
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| STATUS_INVALID)?) as usize)
    }

    /// 读取块数据。
    pub fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        if buffer.len() > FRAME_LEN {
            return Err(STATUS_INVALID);
        }
        self.request(OP_READ, block, buffer.len() as u32)?;
        let written = self.written()?.min(buffer.len());
        buffer[..written].copy_from_slice(&self.frame()[..written]);
        Ok(written)
    }

    /// 写入块数据。
    pub fn write(&mut self, block: u64, data: &[u8]) -> Result<usize, u8> {
        if data.len() > FRAME_LEN {
            return Err(STATUS_INVALID);
        }
        self.frame()[..data.len()].copy_from_slice(data);
        self.request(OP_WRITE, block, data.len() as u32)?;
        self.written()
    }

    /// 刷写。
    pub fn flush(&mut self) -> Result<(), u8> {
        self.request(OP_FLUSH, 0, 0)
    }

    /// 返回容量字节数。
    pub fn capacity(&mut self) -> Result<u64, u8> {
        self.request(OP_CAPACITY, 0, 0)?;
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8]);
        Ok(u64::from_le_bytes(bytes))
    }
}

fn block_status(status: u8) -> BlockError {
    match status {
        STATUS_RANGE => BlockError::OutOfRange,
        STATUS_INVALID => BlockError::InvalidLength,
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
    pub fn open(service: &'static str, own_endpoint: u64) -> Result<Self, u8> {
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
}

/// 在端点上服务块设备请求，直到任务退出或收到退出请求。
pub fn serve(endpoint: u64, disk: &mut dyn BlockDevice, mut frame: Option<&mut [u8]>) {
    let mut buffer = alloc::vec![0_u8; 64 * 1024];
    let mut body = alloc::vec![0_u8; 64 * 1024 - 4];
    let mut response = alloc::vec![0_u8; 64 * 1024];

    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut buffer);
        if label == BLOCK_SHUTDOWN {
            return;
        }
        if label != BLOCK_REQUEST {
            continue;
        }
        let Some(request) = decode_request(&buffer[..length.min(buffer.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        if let Some(frame) = frame.as_deref_mut() {
            let (status, written) = handle_frame(disk, &request, frame);
            let mut length = [0_u8; 8];
            length.copy_from_slice(&(written as u64).to_le_bytes());
            reply(sender, status, &length, &mut response);
        } else {
            let (status, written) = handle(disk, &request, &mut body);
            reply(sender, status, &body[..written], &mut response);
        }
    }
}

fn handle_frame(
    disk: &mut dyn BlockDevice,
    request: &Request<'_>,
    frame: &mut [u8],
) -> (u8, usize) {
    match request.op {
        OP_READ => {
            let length = request.length as usize;
            if length == 0 || length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match disk.read_blocks(request.block, &mut frame[..length]) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => {
            let length = request.length as usize;
            if length == 0 || length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match disk.write_blocks(request.block, &frame[..length]) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_FLUSH => match disk.flush() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_CAPACITY => (STATUS_OK, disk.capacity_bytes() as usize),
        _ => (STATUS_INVALID, 0),
    }
}

fn handle(disk: &mut dyn BlockDevice, request: &Request<'_>, body: &mut [u8]) -> (u8, usize) {
    match request.op {
        OP_READ => {
            let length = request.length as usize;
            if length == 0 || length > body.len() {
                return (STATUS_INVALID, 0);
            }
            match disk.read_blocks(request.block, &mut body[..length]) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => match disk.write_blocks(request.block, request.data) {
            Ok(length) => (STATUS_OK, length),
            Err(error) => (status(error), 0),
        },
        OP_FLUSH => match disk.flush() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_CAPACITY => {
            body[..8].copy_from_slice(&disk.capacity_bytes().to_le_bytes());
            (STATUS_OK, 8)
        }
        _ => (STATUS_INVALID, 0),
    }
}

fn status(error: BlockError) -> u8 {
    match error {
        BlockError::OutOfRange => STATUS_RANGE,
        BlockError::InvalidLength => STATUS_INVALID,
        BlockError::ReadOnly | BlockError::Io => STATUS_ERROR,
    }
}

fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
    }
}
