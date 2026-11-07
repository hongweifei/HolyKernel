//! 服务目录：镜像构建、启动白名单与桌面排序的单一清单。
//!
//! 新增服务时只修改 [`SERVICES`]；构建工具、Root Server 与平台装载从同一
//! 清单读取名称、启动标记和桌面排序。

#![no_std]

use core::iter::Iterator;

/// 服务条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    /// 服务镜像名。
    pub name: &'static str,
    /// 是否在默认启动白名单中。
    pub boot: bool,
    /// 桌面模式排序；数值小的先启动。
    pub desktop_order: u8,
}

macro_rules! services {
    ($(($name:literal, $boot:literal, $desktop:literal)),* $(,)?) => {
        /// 全部服务；顺序即镜像构建与默认装载顺序。
        pub const SERVICES: &[Service] = &[
            $(Service {
                name: $name,
                boot: $boot,
                desktop_order: $desktop,
            }),*
        ];

        /// 全部服务名，供构建工具遍历。
        pub const SERVERS: &[&str] = &[$($name),*];
    };
}

services![
    ("hk-server-demo", true, 9),
    ("hk-server-echo", true, 9),
    ("hk-server-device", true, 1),
    ("hk-server-spin", true, 9),
    ("hk-server-virtio", true, 3),
    ("hk-server-memory", true, 9),
    ("hk-server-holyfs", true, 4),
    ("hk-server-vfs", true, 5),
    ("hk-server-path", true, 6),
    ("hk-server-fault", false, 9),
    ("hk-server-fork", false, 9),
    ("hk-server-latency", false, 9),
    ("hk-server-block", true, 9),
    ("hk-server-virtio-net", true, 9),
    ("hk-server-net", true, 9),
    ("hk-server-netstat", false, 9),
    ("hk-server-netsock", true, 9),
    ("hk-server-console", true, 8),
    ("hk-server-console-test", true, 9),
    ("hk-server-shell", true, 9),
    ("hk-server-display", true, 0),
    ("hk-server-display-test", true, 9),
    ("hk-server-terminal-test", true, 9),
    ("hk-server-input", true, 2),
    ("hk-server-pointer-test", true, 9),
    ("hk-server-holydoc", true, 7),
    ("hk-server-workbench", true, 7),
    ("hk-server-debug", true, 9),
    ("hk-server-taskman", true, 7),
    ("hk-server-dbg", false, 9),
    ("hk-server-display-restart-test", true, 9),
    ("hk-server-checkpoint", true, 9),
    ("hk-server-checkpoint-test", true, 9),
];

/// 按名称查找服务。
pub fn service(name: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|service| service.name == name)
}

/// 判断服务是否在默认启动白名单中。
pub fn boot_service(name: &str) -> bool {
    service(name).is_some_and(|service| service.boot)
}

/// 返回桌面模式排序键；未登记服务排在最后。
pub fn desktop_order(name: &str) -> u8 {
    service(name).map_or(9, |service| service.desktop_order)
}
