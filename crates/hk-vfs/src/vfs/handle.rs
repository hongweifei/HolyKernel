use super::*;

impl Vfs {
    /// 打开文件，返回句柄。
    pub fn open(&mut self, path: &str, flags: OpenFlags) -> Result<FileHandle, VfsError> {
        let path = normalize(path);
        let (instance, node) = match self.resolve(&path) {
            Ok((instance, node)) => {
                let metadata = self.instances[instance]
                    .backend
                    .metadata(node)
                    .map_err(|_| VfsError::Backend)?;
                if metadata.kind == FileType::Directory && flags.write {
                    return Err(VfsError::IsDirectory);
                }
                (instance, node)
            }
            Err(VfsError::NotFound) if flags.create => self.create_node(&path, FileType::File)?,
            Err(error) => return Err(error),
        };

        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(VfsError::Backend)?;
        self.handles.insert(
            handle,
            OpenFile {
                instance,
                node,
                offset: 0,
                flags,
            },
        );
        Ok(FileHandle(handle))
    }

    /// 关闭文件句柄。
    pub fn close(&mut self, handle: FileHandle) -> Result<(), VfsError> {
        self.handles
            .remove(&handle.0)
            .map(|_| ())
            .ok_or(VfsError::NotFound)
    }

    /// 从文件句柄的指定偏移读取。
    pub fn read_at(
        &mut self,
        handle: FileHandle,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let (instance, node) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.read {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.node)
        };
        let backend = &*self.instances[instance].backend;
        if backend.cacheable(node) {
            self.cache.read(backend, instance, node, offset, buffer)
        } else {
            backend.read(node, offset, buffer)
        }
    }

    /// 从文件句柄读取，并推进偏移。
    pub fn read(&mut self, handle: FileHandle, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let offset = self
            .handles
            .get(&handle.0)
            .ok_or(VfsError::NotFound)?
            .offset;
        let read = self.read_at(handle, offset, buffer)?;
        if let Some(open) = self.handles.get_mut(&handle.0) {
            open.offset = open
                .offset
                .checked_add(read as u64)
                .ok_or(VfsError::InvalidOffset)?;
        }
        Ok(read)
    }

    /// 向文件句柄的指定偏移写入。
    pub fn write_at(
        &mut self,
        handle: FileHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        let (instance, node) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.write {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.node)
        };
        let backend = &mut *self.instances[instance].backend;
        if backend.cacheable(node) {
            self.cache.write(backend, instance, node, offset, data)
        } else {
            backend.write(node, offset, data)
        }
    }

    /// 向文件句柄写入，并推进偏移。
    pub fn write(&mut self, handle: FileHandle, data: &[u8]) -> Result<usize, VfsError> {
        let offset = self
            .handles
            .get(&handle.0)
            .ok_or(VfsError::NotFound)?
            .offset;
        let written = self.write_at(handle, offset, data)?;
        if let Some(open) = self.handles.get_mut(&handle.0) {
            open.offset = open
                .offset
                .checked_add(written as u64)
                .ok_or(VfsError::InvalidOffset)?;
        }
        Ok(written)
    }

    /// 读取元数据。
    pub fn metadata(&mut self, path: &str) -> Result<Metadata, VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.metadata(node)
    }

    /// 读取目录项。
    pub fn readdir(&mut self, path: &str) -> Result<Vec<DirEntry>, VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.readdir(node)
    }

    /// 创建目录。
    pub fn create_dir(&mut self, path: &str) -> Result<(), VfsError> {
        self.create_node(path, FileType::Directory).map(|_| ())
    }

    /// 调整文件大小。
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<(), VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.truncate(node, size)?;
        self.cache.invalidate(instance, node);
        Ok(())
    }

    /// 删除文件或空目录。
    pub fn unlink(&mut self, path: &str) -> Result<(), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_node) = self.resolve(&parent)?;
        let node = self.instances[instance]
            .backend
            .unlink(parent_node, &name)?;
        self.dentries.remove(&(instance, parent_node, name));
        self.cache.invalidate(instance, node);
        Ok(())
    }

    /// 刷写全部后端的挂起写入。
    pub fn sync_all(&mut self) -> Result<(), VfsError> {
        for instance in &mut self.instances {
            instance.backend.sync()?;
        }
        Ok(())
    }

    pub(crate) fn create_node(
        &mut self,
        path: &str,
        kind: FileType,
    ) -> Result<(usize, NodeId), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_node) = self.resolve(&parent)?;
        let metadata = self.instances[instance].backend.metadata(parent_node)?;
        if metadata.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        let node = self.instances[instance]
            .backend
            .create(parent_node, &name, kind)?;
        self.dentries.insert((instance, parent_node, name), node);
        Ok((instance, node))
    }
}

pub(crate) fn split_parent(path: &str) -> Result<(String, String), VfsError> {
    let path = normalize(path);
    let (parent, name) = path.rsplit_once('/').ok_or(VfsError::InvalidPath)?;
    if name.is_empty() {
        return Err(VfsError::InvalidPath);
    }
    let parent = if parent.is_empty() {
        String::from("/")
    } else {
        parent.to_string()
    };
    Ok((parent, name.to_string()))
}
