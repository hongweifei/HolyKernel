//! caller-pays 借贷策略。

use crate::CadenceError;

/// 默认借贷深度。
pub const DEFAULT_LENDING_DEPTH: u8 = 2;

/// 最大借贷深度。
pub const MAX_LENDING_DEPTH: u8 = 8;

/// 借贷策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lending {
    enabled: bool,
    max_depth: u8,
}

impl Lending {
    /// 关闭借贷。
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            max_depth: 0,
        }
    }

    /// 启用 caller-pays 借贷，使用默认深度。
    pub const fn caller_pays() -> Self {
        Self {
            enabled: true,
            max_depth: DEFAULT_LENDING_DEPTH,
        }
    }

    /// 以指定深度启用借贷。
    pub const fn with_depth(depth: u8) -> Result<Self, CadenceError> {
        if depth > MAX_LENDING_DEPTH {
            return Err(CadenceError::LendingTooDeep);
        }
        Ok(Self {
            enabled: depth != 0,
            max_depth: depth,
        })
    }

    /// 判断是否启用。
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    /// 返回最大借贷深度。
    pub const fn max_depth(self) -> u8 {
        self.max_depth
    }

    /// 把策略收窄到父策略范围内。
    ///
    /// 父契约关闭借贷时，子契约也必须关闭；子契约的深度不能超过父契约。
    pub const fn narrowed_to(self, parent: Self) -> Result<Self, CadenceError> {
        if !parent.enabled {
            if self.enabled {
                return Err(CadenceError::LendingWidensParent);
            }
            return Ok(Self::disabled());
        }
        if !self.enabled {
            return Ok(Self::disabled());
        }
        if self.max_depth > parent.max_depth {
            return Err(CadenceError::LendingWidensParent);
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_depth() {
        assert_eq!(Lending::with_depth(0), Ok(Lending::disabled()));
        assert_eq!(
            Lending::with_depth(MAX_LENDING_DEPTH),
            Ok(Lending {
                enabled: true,
                max_depth: MAX_LENDING_DEPTH,
            })
        );
        assert_eq!(
            Lending::with_depth(MAX_LENDING_DEPTH + 1).err(),
            Some(CadenceError::LendingTooDeep)
        );
    }

    #[test]
    fn narrows_to_parent() {
        let parent = Lending::with_depth(3).unwrap();
        let child = Lending::with_depth(2).unwrap();
        assert_eq!(child.narrowed_to(parent), Ok(child));
        assert_eq!(
            Lending::with_depth(4).unwrap().narrowed_to(parent).err(),
            Some(CadenceError::LendingWidensParent)
        );
        assert_eq!(
            Lending::caller_pays()
                .narrowed_to(Lending::disabled())
                .err(),
            Some(CadenceError::LendingWidensParent)
        );
        assert_eq!(
            Lending::disabled().narrowed_to(parent),
            Ok(Lending::disabled())
        );
    }
}
