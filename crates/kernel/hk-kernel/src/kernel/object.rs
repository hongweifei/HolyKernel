use super::*;

impl Kernel {
    /// 在指定线程的句柄表中创建端点。
    pub fn create_endpoint(&self, thread: TcbId, rights: Rights) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Endpoint(hk_ipc::Endpoint::new());
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 在指定线程的句柄表中创建设备授权工厂。
    pub fn create_device_grant_factory(
        &self,
        thread: TcbId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::DeviceGrantFactory)),
            rights,
        )
    }

    /// 通过设备授权工厂创建设备授权对象。
    pub fn create_device_grant_authorized(
        &self,
        thread: TcbId,
        factory: Handle,
        grant: DeviceGrant,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        self.resolve(
            thread,
            factory,
            Rights::READ,
            ObjectKind::DeviceGrantFactory,
        )?;
        self.create_device_grant(thread, grant, rights)
    }

    /// 在指定线程的句柄表中创建设备授权对象。
    pub fn create_device_grant(
        &self,
        thread: TcbId,
        grant: DeviceGrant,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::DeviceGrant(grant);
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 在指定线程的句柄表中创建通知。
    pub fn create_notification(
        &self,
        thread: TcbId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Notification(hk_ipc::Notification::new());
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

    /// 解析 DeviceGrant 对象。
    pub fn device_grant(&self, thread: TcbId, handle: Handle) -> Result<DeviceGrant, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::DeviceGrant)?;
        object
            .device_grant()
            .copied()
            .ok_or(KernelError::WrongKind {
                expected: ObjectKind::DeviceGrant,
            })
    }

    /// 复制句柄并收窄权限。
    pub fn duplicate_handle(
        &self,
        thread: TcbId,
        handle: Handle,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        Ok(table.lock().duplicate(handle, rights)?)
    }

    /// 把已有对象的句柄授予指定线程。
    pub fn grant(
        &self,
        thread: TcbId,
        object: ObjectId,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let target = self.upgrade(object)?;
        self.register(thread, target, rights)
    }

    /// 关闭线程句柄表中的一个句柄。
    pub fn close_handle(&self, thread: TcbId, handle: Handle) -> Result<(), KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        let id = {
            let mut table = table.lock();
            let id = table.resolve(handle, Rights::empty())?.id();
            table.close(handle)?;
            id
        };
        self.record(thread.get(), Category::Handle, TRACE_HANDLE_CLOSE, id.get());
        Ok(())
    }

    /// 返回句柄指向的对象标识。
    pub fn handle_id(&self, thread: TcbId, handle: Handle) -> Result<ObjectId, KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        Ok(table.lock().handle_id(handle)?)
    }

    pub(crate) fn register(
        &self,
        thread: TcbId,
        object: Arc<KernelObject>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        let handle = table
            .lock()
            .insert(Arc::clone(&object) as Arc<dyn Object>, rights)?;
        {
            let mut objects = self.objects.lock();
            objects.insert(object.header().id(), Arc::downgrade(&object));
            if objects.len() > 256 && objects.len().is_multiple_of(64) {
                objects.retain(|_, weak| weak.strong_count() > 0);
            }
        }
        self.record(
            thread.get(),
            Category::Handle,
            TRACE_HANDLE_GRANT,
            object.header().id().get(),
        );
        Ok(handle)
    }

    pub(crate) fn resolve(
        &self,
        thread: TcbId,
        handle: Handle,
        required: Rights,
        expected: ObjectKind,
    ) -> Result<(Arc<KernelObject>, ObjectId), KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        let resolved = table.lock().resolve_kind(handle, required, expected)?;
        let id = resolved.id();
        let object = resolved
            .into_any()
            .downcast::<KernelObject>()
            .map_err(|_| KernelError::WrongKind { expected })?;
        Ok((object, id))
    }

    /// 返回句柄实际授予的权限；只校验对象类别，不要求额外权限。
    pub fn handle_rights(
        &self,
        thread: TcbId,
        handle: Handle,
        expected: ObjectKind,
    ) -> Result<Rights, KernelError> {
        let table = self
            .threads
            .lock()
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?
            .handles();
        let resolved = table
            .lock()
            .resolve_kind(handle, Rights::empty(), expected)?;
        Ok(resolved.rights())
    }

    pub(crate) fn upgrade(&self, id: ObjectId) -> Result<Arc<KernelObject>, KernelError> {
        self.objects
            .lock()
            .get(&id)
            .and_then(Weak::upgrade)
            .ok_or(KernelError::StaleObject(id))
    }

    /// 清理已析构对象的注册表项，返回清理数量。
    pub fn collect_garbage(&self) -> usize {
        let mut objects = self.objects.lock();
        let before = objects.len();
        objects.retain(|_, weak| weak.strong_count() > 0);
        before - objects.len()
    }
}
