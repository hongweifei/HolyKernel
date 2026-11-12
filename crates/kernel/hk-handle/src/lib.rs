//! 句柄、对象标识与句柄表。
//!
//! 每张句柄表属于一个 Native Task；表项记录对象、rights 与授予时的对象 epoch。
//! 查找校验 `generation` 与 `epoch`：`close` 递增槽位 generation，`revoke_all`
//! 递增对象 epoch，两者都让旧句柄失效。

#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 每 Task 默认句柄配额。
pub const DEFAULT_HANDLE_QUOTA: usize = 4096;

/// 对象标识，内核范围内唯一。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(u64);

impl ObjectId {
    /// 以数值构造对象标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

static NEXT_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_object_id() -> ObjectId {
    ObjectId(NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed))
}

/// 内核对象类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// 线程控制块。
    Tcb,
    /// 调度主体。
    Task,
    /// 时间契约。
    Cadence,
    /// 调度计量。
    Meter,
    /// 句柄表。
    HandleTable,
    /// 同步 IPC 端点。
    Endpoint,
    /// 异步事件通知。
    Notification,
    /// 中断处理句柄。
    IrqHandler,
    /// 设备授权。
    DeviceGrant,
    /// 设备授权工厂。
    DeviceGrantFactory,
    /// 物理容量能力。
    Capacity,
    /// 物理页。
    Frame,
    /// 类型化能力。
    Capability,
    /// 虚拟映射。
    Region,
}

impl ObjectKind {
    /// 返回类别名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tcb => "tcb",
            Self::Task => "task",
            Self::Cadence => "cadence",
            Self::Meter => "meter",
            Self::HandleTable => "handle-table",
            Self::Endpoint => "endpoint",
            Self::Notification => "notification",
            Self::IrqHandler => "irq-handler",
            Self::DeviceGrant => "device-grant",
            Self::DeviceGrantFactory => "device-grant-factory",
            Self::Capacity => "capacity",
            Self::Frame => "frame",
            Self::Capability => "capability",
            Self::Region => "region",
        }
    }
}

/// 对象头：标识、类别、撤销 epoch 与句柄计数。
#[derive(Debug)]
pub struct ObjectHeader {
    id: ObjectId,
    kind: ObjectKind,
    epoch: AtomicU32,
    handles: AtomicU32,
}

impl ObjectHeader {
    /// 创建对象头，标识由内核全局分配。
    pub fn new(kind: ObjectKind) -> Arc<Self> {
        Arc::new(Self {
            id: allocate_object_id(),
            kind,
            epoch: AtomicU32::new(0),
            handles: AtomicU32::new(0),
        })
    }

    /// 以指定标识创建对象头，用于确定性回放与测试。
    pub fn with_id(id: ObjectId, kind: ObjectKind) -> Arc<Self> {
        Arc::new(Self {
            id,
            kind,
            epoch: AtomicU32::new(0),
            handles: AtomicU32::new(0),
        })
    }

    /// 返回对象标识。
    pub fn id(&self) -> ObjectId {
        self.id
    }

    /// 返回对象类别。
    pub fn kind(&self) -> ObjectKind {
        self.kind
    }

    /// 返回当前 epoch。
    pub fn epoch(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }

    /// 递增 epoch，使该对象此前授予的全部句柄失效。
    pub fn revoke_all(&self) -> u32 {
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 登记一个指向该对象的新句柄。
    pub fn acquire_handle(&self) -> u32 {
        self.handles.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 注销一个指向该对象的句柄；计数已为零时保持为零。
    pub fn release_handle(&self) -> u32 {
        let mut current = self.handles.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(1);
            match self.handles.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }

    /// 返回当前指向该对象的句柄数。
    pub fn handle_count(&self) -> u32 {
        self.handles.load(Ordering::Acquire)
    }
}

/// 句柄表持有的对象。
///
/// 句柄持有对象的强引用：最后一个句柄关闭时对象随之析构。
pub trait Object: Send + Sync + core::any::Any {
    /// 返回对象头。
    fn header(&self) -> &ObjectHeader;

    /// 以类型擦除的 `Arc` 返回对象，供内核还原具体对象类型。
    fn into_any(self: Arc<Self>) -> Arc<dyn core::any::Any + Send + Sync>;
}

bitflags::bitflags! {
    /// 句柄权限。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Rights: u32 {
        /// 读取对象、接收消息、等待通知、查询状态。
        const READ = hk_syscall::RIGHT_READ;
        /// 修改对象、发送消息、映射可写 Frame。
        const WRITE = hk_syscall::RIGHT_WRITE;
        /// 执行代码、映射可执行 Frame。
        const EXECUTE = hk_syscall::RIGHT_EXECUTE;
        /// 通过 IPC 转移句柄。
        const TRANSFER = hk_syscall::RIGHT_TRANSFER;
        /// 配置对象、duplicate、revoke_all。
        const ADMIN = hk_syscall::RIGHT_ADMIN;
    }
}

impl Rights {
    /// 判断 `self` 是否包含 `requested` 的全部权限。
    pub const fn covers(self, requested: Self) -> bool {
        self.bits() & requested.bits() == requested.bits()
    }
}

/// 句柄：槽位下标与 generation。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    index: u32,
    generation: u32,
}

impl Handle {
    /// 以槽位与 generation 构造句柄。
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// 返回槽位下标。
    pub const fn index(self) -> u32 {
        self.index
    }

    /// 返回 generation。
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// 句柄操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleError {
    /// 句柄不存在、generation 不匹配或槽位为空。
    BadHandle,
    /// 对象已被撤销，句柄失效。
    Revoked,
    /// 权限不足。
    PermissionDenied,
    /// 目标权限超出源句柄权限。
    RightsNotSubset,
    /// 句柄配额已满。
    NoHandles,
    /// 配额小于当前在用句柄数。
    InvalidQuota,
    /// 对象类别与期望不符。
    WrongType {
        /// 期望的对象类别。
        expected: ObjectKind,
    },
}

impl fmt::Display for HandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadHandle => formatter.write_str("句柄无效"),
            Self::Revoked => formatter.write_str("句柄已撤销"),
            Self::PermissionDenied => formatter.write_str("句柄权限不足"),
            Self::RightsNotSubset => formatter.write_str("目标权限超出源句柄权限"),
            Self::NoHandles => formatter.write_str("句柄配额已满"),
            Self::InvalidQuota => formatter.write_str("配额小于在用句柄数"),
            Self::WrongType { expected } => {
                write!(formatter, "对象类别不是 {}", expected.name())
            }
        }
    }
}

impl core::error::Error for HandleError {}

/// 查找结果：对象与生效权限。
#[derive(Clone)]
pub struct Resolved {
    object: Arc<dyn Object>,
    rights: Rights,
}

impl Resolved {
    /// 返回对象。
    pub fn object(&self) -> &Arc<dyn Object> {
        &self.object
    }

    /// 返回句柄权限。
    pub fn rights(&self) -> Rights {
        self.rights
    }

    /// 返回对象标识。
    pub fn id(&self) -> ObjectId {
        self.object.header().id()
    }

    /// 返回对象类别。
    pub fn kind(&self) -> ObjectKind {
        self.object.header().kind()
    }

    /// 取出类型擦除的对象，供内核还原具体类型。
    pub fn into_any(self) -> Arc<dyn core::any::Any + Send + Sync> {
        self.object.into_any()
    }
}

impl fmt::Debug for Resolved {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resolved")
            .field("id", &self.id())
            .field("kind", &self.kind())
            .field("rights", &self.rights)
            .finish()
    }
}

struct Slot {
    object: Arc<dyn Object>,
    rights: Rights,
    grant_epoch: u32,
}

/// 每 Task 的句柄表。
pub struct HandleTable {
    slots: Vec<Option<Slot>>,
    generations: Vec<u32>,
    free: Vec<u32>,
    quota: usize,
    live: usize,
}

impl HandleTable {
    /// 以配额创建空句柄表。
    pub fn new(quota: usize) -> Self {
        Self {
            slots: Vec::new(),
            generations: Vec::new(),
            free: Vec::new(),
            quota,
            live: 0,
        }
    }

    /// 以默认配额创建空句柄表。
    pub fn with_default_quota() -> Self {
        Self::new(DEFAULT_HANDLE_QUOTA)
    }

    /// 返回在用句柄数。
    pub fn live(&self) -> usize {
        self.live
    }

    /// 返回配额。
    pub fn quota(&self) -> usize {
        self.quota
    }

    /// 调整配额；新配额小于在用句柄数时失败。
    pub fn set_quota(&mut self, quota: usize) -> Result<(), HandleError> {
        if quota < self.live {
            return Err(HandleError::InvalidQuota);
        }
        self.quota = quota;
        Ok(())
    }

    /// 授予句柄。
    pub fn insert(
        &mut self,
        object: Arc<dyn Object>,
        rights: Rights,
    ) -> Result<Handle, HandleError> {
        if self.live >= self.quota {
            return Err(HandleError::NoHandles);
        }
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.slots.push(None);
                self.generations.push(0);
                (self.slots.len() - 1) as u32
            }
        };
        let generation = self.generations[index as usize];
        let grant_epoch = object.header().epoch();
        object.header().acquire_handle();
        self.slots[index as usize] = Some(Slot {
            object,
            rights,
            grant_epoch,
        });
        self.live += 1;
        Ok(Handle::new(index, generation))
    }

    /// 查找句柄并校验权限、generation 与 epoch。
    pub fn resolve(&self, handle: Handle, required: Rights) -> Result<Resolved, HandleError> {
        let slot = self.slot(handle)?;
        if !slot.rights.covers(required) {
            return Err(HandleError::PermissionDenied);
        }
        if slot.grant_epoch != slot.object.header().epoch() {
            return Err(HandleError::Revoked);
        }
        Ok(Resolved {
            object: Arc::clone(&slot.object),
            rights: slot.rights,
        })
    }

    /// 查找句柄并返回对象标识，不克隆对象引用。
    pub fn handle_id(&self, handle: Handle) -> Result<ObjectId, HandleError> {
        let slot = self.slot(handle)?;
        if slot.grant_epoch != slot.object.header().epoch() {
            return Err(HandleError::Revoked);
        }
        Ok(slot.object.header().id())
    }

    /// 查找句柄并校验对象类别。
    pub fn resolve_kind(
        &self,
        handle: Handle,
        required: Rights,
        expected: ObjectKind,
    ) -> Result<Resolved, HandleError> {
        let resolved = self.resolve(handle, required)?;
        match resolved.kind() {
            kind if kind == expected => Ok(resolved),
            _ => Err(HandleError::WrongType { expected }),
        }
    }

    /// 复制句柄；需要 `ADMIN`，结果权限只能收窄。
    pub fn duplicate(&mut self, handle: Handle, rights: Rights) -> Result<Handle, HandleError> {
        let source = self.resolve(handle, Rights::ADMIN)?;
        if !source.rights.covers(rights) {
            return Err(HandleError::RightsNotSubset);
        }
        self.insert(Arc::clone(&source.object), rights)
    }

    /// 关闭句柄；只影响本表槽位，任意权限均可执行。
    pub fn close(&mut self, handle: Handle) -> Result<(), HandleError> {
        self.release(handle)?;
        Ok(())
    }

    /// 取出句柄用于 IPC 转移；需要 `TRANSFER`，成功后半句柄从本表移除。
    pub fn take(&mut self, handle: Handle, required: Rights) -> Result<Resolved, HandleError> {
        let resolved = self.resolve(handle, required | Rights::TRANSFER)?;
        self.release(handle)?;
        Ok(resolved)
    }

    /// 关闭指向指定对象的全部句柄，用于服务器重启与对象回收。
    pub fn close_all_for(&mut self, id: ObjectId) -> usize {
        let mut closed = 0;
        for index in 0..self.slots.len() {
            let matches = self.slots[index]
                .as_ref()
                .is_some_and(|slot| slot.object.header().id() == id);
            if matches {
                if let Some(slot) = self.slots[index].take() {
                    slot.object.header().release_handle();
                }
                self.generations[index] = self.generations[index].wrapping_add(1);
                self.free.push(index as u32);
                self.live -= 1;
                closed += 1;
            }
        }
        closed
    }

    /// 按谓词吊销对象头，使匹配的句柄在解析时失效；返回吊销数量。
    ///
    /// 句柄槽位保留，`resolve` 通过对象头 epoch 与授予 epoch 不一致判定失效。
    pub fn revoke_matching<F>(&mut self, mut predicate: F) -> usize
    where
        F: FnMut(&Arc<dyn Object>) -> bool,
    {
        let mut revoked = 0;
        for slot in self.slots.iter_mut().flatten() {
            if predicate(&slot.object) {
                slot.object.header().revoke_all();
                revoked += 1;
            }
        }
        revoked
    }

    fn slot(&self, handle: Handle) -> Result<&Slot, HandleError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() || self.generations[index] != handle.generation() {
            return Err(HandleError::BadHandle);
        }
        self.slots[index].as_ref().ok_or(HandleError::BadHandle)
    }

    fn release(&mut self, handle: Handle) -> Result<(), HandleError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() || self.generations[index] != handle.generation() {
            return Err(HandleError::BadHandle);
        }
        let Some(slot) = self.slots[index].take() else {
            return Err(HandleError::BadHandle);
        };
        slot.object.header().release_handle();
        self.generations[index] = self.generations[index].wrapping_add(1);
        self.free.push(index as u32);
        self.live -= 1;
        Ok(())
    }
}

impl Drop for HandleTable {
    fn drop(&mut self) {
        for slot in self.slots.iter_mut() {
            if let Some(slot) = slot.take() {
                slot.object.header().release_handle();
            }
        }
    }
}

impl fmt::Debug for HandleTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HandleTable")
            .field("live", &self.live)
            .field("quota", &self.quota)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestObject {
        header: Arc<ObjectHeader>,
    }

    impl TestObject {
        fn new(kind: ObjectKind) -> Arc<Self> {
            Arc::new(Self {
                header: ObjectHeader::new(kind),
            })
        }
    }

    impl Object for TestObject {
        fn header(&self) -> &ObjectHeader {
            &self.header
        }

        fn into_any(self: Arc<Self>) -> Arc<dyn core::any::Any + Send + Sync> {
            self
        }
    }

    fn object() -> Arc<TestObject> {
        TestObject::new(ObjectKind::Endpoint)
    }

    #[test]
    fn grants_and_resolves_handles() {
        let mut table = HandleTable::with_default_quota();
        let object = object();
        let id = object.header().id();

        let handle = table
            .insert(object, Rights::READ | Rights::WRITE)
            .expect("授予成功");
        assert_eq!(table.live(), 1);

        let resolved = table.resolve(handle, Rights::READ).expect("查找成功");
        assert_eq!(resolved.id(), id);
        assert_eq!(resolved.kind(), ObjectKind::Endpoint);
        assert_eq!(resolved.rights(), Rights::READ | Rights::WRITE);
    }

    #[test]
    fn rejects_insufficient_rights() {
        let mut table = HandleTable::with_default_quota();
        let handle = table.insert(object(), Rights::READ).expect("授予成功");

        assert_eq!(
            table.resolve(handle, Rights::WRITE).err(),
            Some(HandleError::PermissionDenied)
        );
        assert_eq!(
            table
                .resolve_kind(handle, Rights::READ, ObjectKind::Capability)
                .err(),
            Some(HandleError::WrongType {
                expected: ObjectKind::Capability
            })
        );
    }

    #[test]
    fn close_invalidates_generation() {
        let mut table = HandleTable::with_default_quota();
        let handle = table.insert(object(), Rights::READ).expect("授予成功");

        table.close(handle).expect("关闭成功");
        assert_eq!(table.live(), 0);
        assert_eq!(
            table.resolve(handle, Rights::READ).err(),
            Some(HandleError::BadHandle)
        );

        let replacement = table.insert(object(), Rights::READ).expect("再次授予");
        assert_eq!(replacement.index(), handle.index());
        assert_ne!(replacement.generation(), handle.generation());
    }

    #[test]
    fn tracks_live_handle_count() {
        let mut first = HandleTable::new(4);
        let mut second = HandleTable::new(4);
        let object = object();
        let header = Arc::clone(&object.header);

        let handle = first
            .insert(object, Rights::READ | Rights::ADMIN | Rights::TRANSFER)
            .expect("首个句柄");
        let duplicate = first.duplicate(handle, Rights::READ).expect("复制句柄");
        assert_eq!(header.handle_count(), 2);

        let moved = first.take(handle, Rights::READ).expect("转移句柄");
        let _granted = second
            .insert(moved.object().clone(), moved.rights())
            .expect("接收句柄");
        assert_eq!(header.handle_count(), 2);

        first.close(duplicate).expect("关闭复制句柄");
        assert_eq!(header.handle_count(), 1);

        drop(first);
        assert_eq!(header.handle_count(), 1, "另一张表的句柄仍然存在");
        drop(second);
        assert_eq!(header.handle_count(), 0);
    }

    #[test]
    fn duplicate_narrows_rights_only() {
        let mut table = HandleTable::with_default_quota();
        let handle = table
            .insert(object(), Rights::READ | Rights::WRITE | Rights::ADMIN)
            .expect("授予成功");

        let narrowed = table.duplicate(handle, Rights::READ).expect("复制成功");
        assert_eq!(
            table.resolve(narrowed, Rights::WRITE).err(),
            Some(HandleError::PermissionDenied)
        );
        assert_eq!(
            table.duplicate(handle, Rights::EXECUTE).err(),
            Some(HandleError::RightsNotSubset)
        );
    }

    #[test]
    fn duplicate_requires_admin() {
        let mut table = HandleTable::with_default_quota();
        let handle = table
            .insert(object(), Rights::READ | Rights::WRITE)
            .expect("授予成功");

        assert_eq!(
            table.duplicate(handle, Rights::READ).err(),
            Some(HandleError::PermissionDenied)
        );
    }

    #[test]
    fn quota_limits_grants() {
        let mut table = HandleTable::new(2);
        table.insert(object(), Rights::READ).expect("第一个句柄");
        table.insert(object(), Rights::READ).expect("第二个句柄");

        assert_eq!(
            table.insert(object(), Rights::READ).err(),
            Some(HandleError::NoHandles)
        );
        assert_eq!(table.set_quota(1).err(), Some(HandleError::InvalidQuota));
        assert_eq!(table.set_quota(4), Ok(()));
        assert!(table.insert(object(), Rights::READ).is_ok());
    }

    #[test]
    fn revocation_invalidates_all_handles() {
        let mut table = HandleTable::with_default_quota();
        let object = object();
        let header = Arc::clone(&object.header);
        let handle = table
            .insert(object, Rights::READ | Rights::ADMIN)
            .expect("授予成功");
        table.duplicate(handle, Rights::READ).expect("复制成功");

        header.revoke_all();
        assert_eq!(
            table.resolve(handle, Rights::READ).err(),
            Some(HandleError::Revoked)
        );
    }

    #[test]
    fn take_moves_handle_out_of_table() {
        let mut table = HandleTable::with_default_quota();
        let handle = table
            .insert(object(), Rights::READ | Rights::TRANSFER)
            .expect("授予成功");

        let moved = table.take(handle, Rights::READ).expect("转移成功");
        assert_eq!(table.live(), 0);
        assert_eq!(
            table.resolve(handle, Rights::READ).err(),
            Some(HandleError::BadHandle)
        );

        let mut receiver = HandleTable::with_default_quota();
        let granted = receiver
            .insert(moved.object().clone(), moved.rights())
            .expect("接收方授予成功");
        assert!(receiver.resolve(granted, Rights::READ).is_ok());
    }

    #[test]
    fn transfer_requires_transfer_right() {
        let mut table = HandleTable::with_default_quota();
        let handle = table.insert(object(), Rights::READ).expect("授予成功");

        assert_eq!(
            table.take(handle, Rights::READ).err(),
            Some(HandleError::PermissionDenied)
        );
    }

    #[test]
    fn closes_all_handles_of_object() {
        let mut table = HandleTable::with_default_quota();
        let object = object();
        let id = object.header().id();
        table
            .insert(
                Arc::clone(&object) as Arc<dyn Object>,
                Rights::READ | Rights::ADMIN,
            )
            .expect("第一个句柄");
        table.insert(object, Rights::READ).expect("第二个句柄");

        assert_eq!(table.close_all_for(id), 2);
        assert_eq!(table.live(), 0);
    }

    #[test]
    fn handle_survives_until_last_reference_closes() {
        let mut table = HandleTable::with_default_quota();
        let first = table.insert(object(), Rights::READ).expect("第一个句柄");
        let second = table.insert(object(), Rights::READ).expect("第二个句柄");

        table.close(first).expect("关闭第一个");
        assert!(table.resolve(second, Rights::READ).is_ok());
    }
}
