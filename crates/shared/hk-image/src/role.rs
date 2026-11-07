//! 服务角色元数据：服务二进制把它链接进专用段，构建镜像时读入任务记录。

/// 服务角色元数据所在的链接段。
pub const SERVICE_ROLE_SECTION: &str = ".hk_service";

/// 把服务角色元数据链接到镜像段。
#[macro_export]
macro_rules! declare_service_role {
    ($name:ident, $role:expr) => {
        #[used]
        #[unsafe(link_section = ".hk_service")]
        static $name: $crate::ServiceRole = $role;
    };
}

/// 服务角色元数据。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceRole {
    /// 固定魔数。
    pub magic: u32,
    /// 角色位图。
    pub role: u32,
}

impl ServiceRole {
    /// 元数据魔数。
    pub const MAGIC: u32 = 0x484B_5352;
    /// 设备管理服务角色。
    pub const DEVICE: u32 = 1;

    /// 构建设备管理服务角色。
    pub const fn device() -> Self {
        Self {
            magic: Self::MAGIC,
            role: Self::DEVICE,
        }
    }

    /// 从段字节解析角色位；魔数不符或长度不足时返回 0。
    pub fn parse(bytes: &[u8]) -> u32 {
        let [a, b, c, d, e, f, g, h, ..] = bytes else {
            return 0;
        };
        if u32::from_le_bytes([*a, *b, *c, *d]) != Self::MAGIC {
            return 0;
        }
        u32::from_le_bytes([*e, *f, *g, *h])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_declared_role() {
        let role = ServiceRole::device();
        let bytes = [role.magic.to_le_bytes(), role.role.to_le_bytes()].concat();
        assert_eq!(ServiceRole::parse(&bytes), ServiceRole::DEVICE);
        assert_eq!(ServiceRole::parse(&[]), 0);
        assert_eq!(ServiceRole::parse(&[0, 0, 0, 0, 1, 0, 0, 0]), 0);
    }
}
