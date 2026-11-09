//! 用户地址空间的通用包装。
//!
//! 架构后端只提供页表根、激活与映射机制；上层通过本类型统一持有后端。

use core::ops::{Deref, DerefMut};

/// 架构页表后端上的用户地址空间。
#[derive(Clone, Copy, Debug)]
pub struct UserSpace<B> {
    backend: B,
}

impl<B> UserSpace<B> {
    /// 包装一个页表后端。
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    /// 返回后端引用。
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// 返回后端可变引用。
    pub const fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// 取出后端。
    pub fn into_backend(self) -> B {
        self.backend
    }
}

impl<B> Deref for UserSpace<B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.backend
    }
}

impl<B> DerefMut for UserSpace<B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.backend
    }
}
