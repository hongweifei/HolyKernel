use super::*;

impl Vfs {
    /// 在挂载点安装 Holy 后端。
    pub fn mount(&mut self, target: &str, backend: Box<dyn Backend>) -> Result<(), VfsError> {
        let target = normalize(target);
        let root = backend.root();
        if target == "/" {
            self.dentries.clear();
            self.cache.clear();
            self.instances[0] = Instance { backend, root };
            return Ok(());
        }
        if self.mounts.iter().any(|mount| mount.target == target) {
            return Err(VfsError::AlreadyExists);
        }

        let instance = self.instances.len();
        self.dentries.clear();
        self.cache.clear();
        self.instances.push(Instance { backend, root });
        self.mounts.push(Mount { target, instance });
        Ok(())
    }

    /// 在挂载点安装路径后端；由 [`PathObject`] 适配。
    pub fn mount_path<P: PathBackend + 'static>(
        &mut self,
        target: &str,
        backend: P,
    ) -> Result<(), VfsError> {
        self.mount(target, Box::new(PathObject::new(backend)))
    }

    /// 卸载挂载点。
    pub fn umount(&mut self, target: &str) -> Result<(), VfsError> {
        let target = normalize(target);
        if target == "/" {
            return Err(VfsError::ReadOnly);
        }
        let index = self
            .mounts
            .iter()
            .position(|mount| mount.target == target)
            .ok_or(VfsError::NotFound)?;
        self.mounts.remove(index);
        self.dentries.clear();
        self.cache.clear();
        Ok(())
    }

    /// 返回挂载点。
    pub fn mounts(&self) -> Vec<String> {
        let mut mounts = alloc::vec![String::from("/")];
        for mount in &self.mounts {
            mounts.push(mount.target.clone());
        }
        mounts
    }

    pub(crate) fn resolve(&mut self, path: &str) -> Result<(usize, DocId), VfsError> {
        let path = normalize(path);
        let mut instance = 0;
        let mut document = self.instances[0].root;
        if let Some((mounted, root)) = self.mount_exact("/") {
            instance = mounted;
            document = root;
        }

        let mut prefix = String::new();
        for component in path.split('/').filter(|component| !component.is_empty()) {
            prefix.push('/');
            prefix.push_str(component);
            if let Some((mounted, root)) = self.mount_exact(&prefix) {
                instance = mounted;
                document = root;
                continue;
            }
            let key = (instance, document, component.to_string());
            if let Some(cached) = self.dentries.get(&key).copied() {
                document = cached;
                continue;
            }
            let found = self.instances[instance]
                .backend
                .resolve(document, component)
                .map_err(map_fs_error)?;
            if self.dentries.len() >= DENTRY_LIMIT {
                self.dentries.clear();
            }
            self.dentries.insert(key, found);
            document = found;
        }
        Ok((instance, document))
    }

    pub(crate) fn mount_exact(&self, target: &str) -> Option<(usize, DocId)> {
        if target == "/" {
            return Some((0, self.instances[0].root));
        }
        self.mounts
            .iter()
            .find(|mount| mount.target == target)
            .map(|mount| {
                let instance = &self.instances[mount.instance];
                (mount.instance, instance.root)
            })
    }
}
