//! 服务二进制与镜像模块清单。

/// 随镜像构建和打包的服务；顺序即启动装载顺序。
pub use hk_catalog::SERVERS;

/// 随镜像打包的 HolyShell 脚本。
pub const SCRIPT_MODULES: &[&str] = &["boot.hsh", "services.hsh"];

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn catalog_matches_workspace_services() {
        let root = crate::workspace::root();
        let mut packages = Vec::new();
        for directory in ["servers", "clients", "demos"] {
            let entries = fs::read_dir(root.join(directory)).expect("读取服务目录");
            for entry in entries {
                let path = entry.expect("读取服务项").path().join("Cargo.toml");
                if !path.is_file() {
                    continue;
                }
                let text = fs::read_to_string(&path).expect("读取 Cargo.toml");
                let manifest: toml::Value = text.parse().expect("解析 Cargo.toml");
                let name = manifest["package"]["name"].as_str().expect("package.name");
                packages.push(name.to_string());
            }
        }
        packages.sort();
        let mut catalog: Vec<String> = SERVERS.iter().map(|name| String::from(*name)).collect();
        catalog.sort();
        assert_eq!(catalog, packages);
    }
}
