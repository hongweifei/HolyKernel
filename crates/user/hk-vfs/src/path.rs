//! 路径规范化。

use alloc::string::String;
use alloc::vec::Vec;

/// 规范化路径：统一为绝对路径，折叠空组件、`.` 与 `..`。
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }

    let mut normalized = String::new();
    for part in parts {
        normalized.push('/');
        normalized.push_str(part);
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_components() {
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("/usr/./bin/../lib"), "/usr/lib");
        assert_eq!(normalize("tmp//file"), "/tmp/file");
        assert_eq!(normalize("/../../tmp"), "/tmp");
        assert_eq!(normalize(""), "/");
    }
}
