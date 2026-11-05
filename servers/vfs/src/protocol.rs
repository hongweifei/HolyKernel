//! VFS 请求处理与回复编码。

use hk_server_api::send_data;
use hk_server_api::vfs::{
    OP_CLOSE, OP_LIST, OP_MKDIR, OP_MOUNT_TMPFS, OP_MOUNTS, OP_OPEN, OP_READ, OP_READ_FRAME,
    OP_STAT, OP_SYNC, OP_TRUNCATE, OP_UMOUNT, OP_UNLINK, OP_WRITE, OP_WRITE_FRAME, OPEN_CREATE,
    OPEN_READ, OPEN_WRITE, Request, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_NOT_FOUND,
    STATUS_OK, STATUS_PERMISSION, VFS_RESPONSE, encode_response,
};
use hk_vfs::{FileType, OpenFlags, Tmpfs, Vfs, VfsError};

use crate::sessions::Session;

#[cfg(target_os = "none")]
fn frame_length(data: &[u8]) -> Option<usize> {
    Some(u64::from_le_bytes(data.get(..8)?.try_into().ok()?) as usize)
}

#[cfg(target_os = "none")]
pub(crate) fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, VFS_RESPONSE, &response[..length]);
    }
}

#[cfg(target_os = "none")]
pub(crate) fn handle(
    filesystem: &mut Vfs,
    session: &mut Session,
    request: &Request<'_>,
    body: &mut [u8],
    frame: Option<&mut [u8]>,
) -> (u8, usize) {
    match request.op {
        OP_OPEN => {
            let mut flags = OpenFlags::default();
            flags.read = request.arg0 & OPEN_READ != 0;
            flags.write = request.arg0 & OPEN_WRITE != 0;
            flags.create = request.arg0 & OPEN_CREATE != 0;
            match filesystem.open(request.path, flags) {
                Ok(handle) => {
                    let id = session.insert(handle);
                    body[..8].copy_from_slice(&id.to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_CLOSE => {
            let Some(handle) = session.remove(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.close(handle) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_READ => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.read_at(handle, request.arg1, body) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.write_at(handle, request.arg1, request.data) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_FRAME => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            let Some(frame) = frame else {
                return (STATUS_INVALID, 0);
            };
            let Some(length) = frame_length(request.data) else {
                return (STATUS_INVALID, 0);
            };
            if length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match filesystem.read_at(handle, request.arg1, &mut frame[..length]) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_FRAME => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            let Some(frame) = frame else {
                return (STATUS_INVALID, 0);
            };
            let Some(length) = frame_length(request.data) else {
                return (STATUS_INVALID, 0);
            };
            if length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match filesystem.write_at(handle, request.arg1, &frame[..length]) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_STAT => match filesystem.metadata(request.path) {
            Ok(metadata) => {
                body[0] = match metadata.kind {
                    FileType::File => 1,
                    FileType::Directory => 2,
                };
                body[1..9].copy_from_slice(&metadata.size.to_le_bytes());
                (STATUS_OK, 9)
            }
            Err(error) => (status(error), 0),
        },
        OP_TRUNCATE => match filesystem.truncate(request.path, request.arg0) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_UNLINK => match filesystem.unlink(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_LIST => match filesystem.readdir(request.path) {
            Ok(entries) => {
                let mut length = 0;
                for entry in entries {
                    let bytes = entry.name.as_bytes();
                    if length + bytes.len() + 1 > body.len() {
                        return (STATUS_ERROR, 0);
                    }
                    body[length..length + bytes.len()].copy_from_slice(bytes);
                    length += bytes.len();
                    body[length] = b'\n';
                    length += 1;
                }
                (STATUS_OK, length)
            }
            Err(error) => (status(error), 0),
        },
        OP_MKDIR => match filesystem.create_dir(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_MOUNT_TMPFS => {
            let _ = filesystem.create_dir(request.path);
            match filesystem.mount_path(request.path, Tmpfs::new()) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_UMOUNT => match filesystem.umount(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_SYNC => match filesystem.sync_all() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_MOUNTS => {
            let mut length = 0;
            for mount in filesystem.mounts() {
                let bytes = mount.as_bytes();
                if length + bytes.len() + 1 > body.len() {
                    return (STATUS_ERROR, 0);
                }
                body[length..length + bytes.len()].copy_from_slice(bytes);
                length += bytes.len();
                body[length] = b'\n';
                length += 1;
            }
            (STATUS_OK, length)
        }
        _ => (STATUS_INVALID, 0),
    }
}
#[cfg(target_os = "none")]
fn status(error: VfsError) -> u8 {
    match error {
        VfsError::NotFound => STATUS_NOT_FOUND,
        VfsError::InvalidPath => STATUS_INVALID,
        VfsError::PermissionDenied => STATUS_PERMISSION,
        VfsError::AlreadyExists => STATUS_EXISTS,
        _ => STATUS_ERROR,
    }
}
