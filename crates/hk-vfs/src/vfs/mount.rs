use super::*;

impl Vfs {
    /// 在挂载点安装后端。
    pub fn mount(&mut self, target: &str, backend: Box<dyn Backend>) -> Result<(), VfsError> {
        let target = normalize(target);
        if target == "/" {
            self.dentries.clear();
            self.cache.clear();
            let root = backend.root();
            self.instances[0] = Instance { backend, root };
            return Ok(());
        }
        if self.mounts.iter().any(|mount| mount.target == target) {
            return Err(VfsError::AlreadyExists);
        }

        let instance = self.instances.len();
        let root = backend.root();
        self.dentries.clear();
        self.cache.clear();
        self.instances.push(Instance { backend, root });
        self.mounts.push(Mount { target, instance });
        Ok(())
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

    pub(crate) fn resolve(&mut self, path: &str) -> Result<(usize, NodeId), VfsError> {
        let path = normalize(path);
        let mut instance = 0;
        let mut node = self.instances[0].root;
        if let Some((mounted, root)) = self.mount_exact("/") {
            instance = mounted;
            node = root;
        }

        let mut prefix = String::new();
        for component in path.split('/').filter(|component| !component.is_empty()) {
            prefix.push('/');
            prefix.push_str(component);
            if let Some((mounted, root)) = self.mount_exact(&prefix) {
                instance = mounted;
                node = root;
                continue;
            }
            let key = (instance, node, component.to_string());
            if let Some(cached) = self.dentries.get(&key).copied() {
                node = cached;
                continue;
            }
            let found = self.instances[instance].backend.lookup(node, component)?;
            if self.dentries.len() >= DENTRY_LIMIT {
                self.dentries.clear();
            }
            self.dentries.insert(key, found);
            node = found;
        }
        Ok((instance, node))
    }

    pub(crate) fn mount_exact(&self, target: &str) -> Option<(usize, NodeId)> {
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
