//! 块协议的服务端循环与远端块设备客户端。
//!
//! 协议编码在 [`hk_server_api::block`]；本 crate 把协议接到
//! [`hk_block::BlockDevice`]，供 Block Server、virtio 驱动服务与 VFS 复用。

#![no_std]

use core::cell::RefCell;

use hk_block::{BlockDevice, BlockError, SECTOR_SIZE};
use hk_server_api::block::{
    BLOCK_REQUEST, BLOCK_RESPONSE, BlockClient, OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE, Request,
    STATUS_ERROR, STATUS_INVALID, STATUS_OK, STATUS_RANGE, decode_request, encode_response,
};
use hk_server_api::{recv_data, send_data};

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

/// 在端点上服务块设备请求，直到任务退出。
pub fn serve(endpoint: u64, disk: &mut dyn BlockDevice) -> ! {
    let mut buffer = [0_u8; 8192];
    let mut body = [0_u8; 8192];
    let mut response = [0_u8; 8192];

    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut buffer);
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
