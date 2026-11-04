use super::*;

impl Kernel {
    /// 在指定线程的句柄表中创建 Untyped 对象。
    pub fn create_untyped(
        &self,
        thread: TcbId,
        region: PhysRange,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::Untyped(Mutex::new(Untyped::new(region)));
        self.register(thread, Arc::new(KernelObject::new(payload)), rights)
    }

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

    /// 在指定线程的句柄表中创建地址空间。
    pub fn create_vspace(
        &self,
        thread: TcbId,
        mmu: &dyn Mmu,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let payload = ObjectPayload::VSpace(VSpace::new(mmu.create_vspace()));
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

    /// 从 Untyped 切分页帧并授予句柄。
    pub fn allocate_frame(
        &self,
        thread: TcbId,
        untyped: Handle,
        len: usize,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let (object, _) = self.resolve(thread, untyped, Rights::WRITE, ObjectKind::Untyped)?;
        let frame = {
            let untyped = object.untyped().ok_or(KernelError::WrongKind {
                expected: ObjectKind::Untyped,
            })?;
            untyped.lock().retype_frame(len)?
        };
        let header = frame.header_arc().clone();
        let payload = ObjectPayload::Frame(frame);
        self.register(
            thread,
            Arc::new(KernelObject::with_header(header, payload)),
            rights,
        )
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
        let id = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            let id = tcb.handles().resolve(handle, Rights::empty())?.id();
            tcb.handles_mut().close(handle)?;
            id
        };
        self.record(thread.get(), Category::Handle, TRACE_HANDLE_CLOSE, id.get());
        Ok(())
    }

    /// 返回句柄指向的对象标识。
    pub fn handle_id(&self, thread: TcbId, handle: Handle) -> Result<ObjectId, KernelError> {
        let threads = self.threads.lock();
        let tcb = threads
            .get(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        Ok(tcb.handles().resolve(handle, Rights::empty())?.id())
    }

    pub(crate) fn register(
        &self,
        thread: TcbId,
        object: Arc<KernelObject>,
        rights: Rights,
    ) -> Result<Handle, KernelError> {
        let handle = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.handles_mut()
                .insert(Arc::clone(&object) as Arc<dyn Object>, rights)?
        };
        self.objects
            .lock()
            .insert(object.header().id(), Arc::downgrade(&object));
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
        let id = {
            let threads = self.threads.lock();
            let tcb = threads
                .get(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.handles().resolve_kind(handle, required, expected)?.id()
        };
        Ok((self.upgrade(id)?, id))
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
