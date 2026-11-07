use super::*;

impl Vfs {
    /// 打开文档，返回句柄。
    pub fn open(&mut self, path: &str, flags: OpenFlags) -> Result<FileHandle, VfsError> {
        let path = normalize(path);
        let (instance, document, info) = match self.resolve(&path) {
            Ok((instance, document)) => {
                let info = self.instances[instance]
                    .backend
                    .document(document)
                    .map_err(map_fs_error)?;
                if info.kind == hk_fs::DocumentKind::Collection && flags.write {
                    return Err(VfsError::IsDirectory);
                }
                (instance, document, info)
            }
            Err(VfsError::NotFound) if flags.create => {
                let (instance, document) = self.create_node(&path, hk_fs::DocumentKind::Content)?;
                let info = self.instances[instance]
                    .backend
                    .document(document)
                    .map_err(map_fs_error)?;
                (instance, document, info)
            }
            Err(error) => return Err(error),
        };

        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(VfsError::Backend)?;
        self.handles.insert(
            handle,
            OpenFile {
                instance,
                document,
                revision: info.head,
                size: info.size,
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
        let (instance, document, revision, size) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.read {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.document, open.revision, open.size)
        };
        let backend = &*self.instances[instance].backend;
        let read = if backend.cacheable(document) {
            self.cache
                .read(backend, instance, document, revision, size, offset, buffer)?
        } else {
            backend
                .read(document, Some(revision), offset, buffer)
                .map_err(map_fs_error)?
        };
        Ok(read)
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
        let (instance, document) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.write {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.document)
        };
        let backend = &mut *self.instances[instance].backend;
        let written = if backend.cacheable(document) {
            self.cache
                .write(backend, instance, document, offset, data)?
        } else {
            backend
                .write(document, offset, data)
                .map_err(map_fs_error)?
        };
        let info = backend.document(document).map_err(map_fs_error)?;
        if let Some(open) = self.handles.get_mut(&handle.0) {
            open.revision = info.head;
            open.size = info.size;
        }
        Ok(written)
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
        let (instance, document) = self.resolve(path)?;
        let info = self.instances[instance]
            .backend
            .document(document)
            .map_err(map_fs_error)?;
        Ok(Metadata {
            kind: file_type(info.kind),
            size: info.size,
        })
    }

    /// 读取目录项。
    pub fn readdir(&mut self, path: &str) -> Result<Vec<DirEntry>, VfsError> {
        let (instance, document) = self.resolve(path)?;
        let entries = self.instances[instance]
            .backend
            .enumerate(document)
            .map_err(map_fs_error)?;
        Ok(entries
            .into_iter()
            .map(|entry| DirEntry {
                name: entry.name,
                kind: file_type(entry.kind),
            })
            .collect())
    }

    /// 创建目录。
    pub fn create_dir(&mut self, path: &str) -> Result<(), VfsError> {
        self.create_node(path, hk_fs::DocumentKind::Collection)
            .map(|_| ())
    }

    /// 调整文件大小。
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<(), VfsError> {
        let (instance, document) = self.resolve(path)?;
        self.instances[instance]
            .backend
            .truncate(document, size)
            .map_err(map_fs_error)?;
        self.cache.invalidate(instance, document);
        if self
            .handles
            .values()
            .any(|open| open.instance == instance && open.document == document)
        {
            let info = self.instances[instance]
                .backend
                .document(document)
                .map_err(map_fs_error)?;
            for open in self.handles.values_mut() {
                if open.instance == instance && open.document == document {
                    open.revision = info.head;
                    open.size = info.size;
                }
            }
        }
        Ok(())
    }

    /// 删除文件或空目录。
    pub fn unlink(&mut self, path: &str) -> Result<(), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_document) = self.resolve(&parent)?;
        let document = self.instances[instance]
            .backend
            .unbind(parent_document, &name)
            .map_err(map_fs_error)?;
        self.dentries.remove(&(instance, parent_document, name));
        self.cache.invalidate(instance, document);
        Ok(())
    }

    /// 刷写全部后端的挂起写入。
    pub fn sync_all(&mut self) -> Result<(), VfsError> {
        for instance in &mut self.instances {
            instance.backend.sync().map_err(map_fs_error)?;
        }
        Ok(())
    }

    pub(crate) fn create_node(
        &mut self,
        path: &str,
        kind: hk_fs::DocumentKind,
    ) -> Result<(usize, DocId), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_document) = self.resolve(&parent)?;
        let info = self.instances[instance]
            .backend
            .document(parent_document)
            .map_err(map_fs_error)?;
        if info.kind != hk_fs::DocumentKind::Collection {
            return Err(VfsError::NotDirectory);
        }
        let document = self.instances[instance]
            .backend
            .create(parent_document, &name, kind)
            .map_err(map_fs_error)?;
        self.dentries
            .insert((instance, parent_document, name), document);
        Ok((instance, document))
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

fn file_type(kind: hk_fs::DocumentKind) -> FileType {
    match kind {
        hk_fs::DocumentKind::Content => FileType::File,
        hk_fs::DocumentKind::Collection => FileType::Directory,
    }
}
