//! 块协议的服务端循环与远端块设备客户端。
//!
//! 协议编码在 [`hk_server_api::block`]；本模块把协议接到 [`BlockDevice`]，
//! 供 Block Server、virtio 驱动服务与 VFS 复用。

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::{BlockDevice, BlockError, SECTOR_SIZE};
use hk_server_api::block::{
    BLOCK_REQUEST, BLOCK_RESPONSE, BLOCK_SHUTDOWN, OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE,
    RESPONSE_HEADER, Request, STATUS_ERROR, STATUS_INVALID, STATUS_OK, STATUS_RANGE,
    decode_request, decode_response, encode_request, encode_response,
};
use hk_server_api::{recv_data, recv_data_from, send_data, service_endpoint};

/// 块协议客户端；缓冲位于堆上，避免大消息占用服务栈。
pub struct BlockClient {
    service: &'static str,
    endpoint: u64,
    own_endpoint: u64,
    request: Vec<u8>,
    response: Vec<u8>,
}

impl BlockClient {
    /// 构造客户端。
    pub fn new(service: &'static str, own_endpoint: u64) -> Self {
        Self {
            service,
            endpoint: 0,
            own_endpoint,
            request: vec![0_u8; 64 * 1024],
            response: vec![0_u8; 64 * 1024],
        }
    }

    fn exchange(&mut self, op: u8, block: u64, length: u32, data: &[u8]) -> Result<usize, u8> {
        let Some(request_length) = encode_request(&mut self.request, op, block, length, data)
        else {
            return Err(STATUS_INVALID);
        };
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        self.endpoint = endpoint;
        if send_data(
            self.endpoint,
            BLOCK_REQUEST,
            &self.request[..request_length],
        ) == u64::MAX
        {
            return Err(STATUS_ERROR);
        }
        let (label, _word, _sender, response_length) =
            recv_data_from(self.own_endpoint, self.endpoint, &mut self.response);
        if label != BLOCK_RESPONSE {
            return Err(STATUS_ERROR);
        }
        let Some((status, body)) = decode_response(&self.response[..response_length]) else {
            return Err(STATUS_INVALID);
        };
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(body.len())
    }

    /// 读取块数据。
    pub fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        let length = buffer.len().min(u32::MAX as usize) as u32;
        let copied = self.exchange(OP_READ, block, length, &[])?;
        let copy = copied.min(buffer.len());
        buffer[..copy].copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + copy]);
        Ok(copy)
    }

    /// 写入块数据。
    pub fn write(&mut self, block: u64, data: &[u8]) -> Result<usize, u8> {
        self.exchange(OP_WRITE, block, data.len() as u32, data)
    }

    /// 刷写。
    pub fn flush(&mut self) -> Result<(), u8> {
        self.exchange(OP_FLUSH, 0, 0, &[]).map(|_| ())
    }

    /// 返回容量字节数。
    pub fn capacity(&mut self) -> Result<u64, u8> {
        let length = self.exchange(OP_CAPACITY, 0, 0, &[])?;
        if length < 8 {
            return Err(STATUS_INVALID);
        }
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
pub fn serve(endpoint: u64, disk: &mut dyn BlockDevice) {
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
        let (status, written) = handle(disk, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
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
