//! 终端样式：默认色、16/256 色、SGR 属性与单元格。

/// 终端背景色。
pub(crate) const BACKGROUND: u32 = 0x0010_1010;
/// 终端前景色。
pub(crate) const FOREGROUND: u32 = 0x00D0_D0D0;
/// 光标颜色。
pub(crate) const CURSOR: u32 = 0x0060_A0E0;

/// 粗体。
pub(crate) const FLAG_BOLD: u8 = 1 << 0;
/// 暗淡。
pub(crate) const FLAG_DIM: u8 = 1 << 1;
/// 下划线。
pub(crate) const FLAG_UNDERLINE: u8 = 1 << 2;
/// 反显。
pub(crate) const FLAG_REVERSE: u8 = 1 << 3;

/// ANSI 基本 16 色。
const PALETTE: [u32; 16] = [
    0x0000_0000,
    0x0080_0000,
    0x0000_8000,
    0x0080_8000,
    0x0000_0080,
    0x0080_0080,
    0x0000_8080,
    0x00C0_C0C0,
    0x0080_8080,
    0x00FF_0000,
    0x0000_FF00,
    0x00FF_FF00,
    0x0000_00FF,
    0x00FF_00FF,
    0x0000_FFFF,
    0x00FF_FFFF,
];

/// 单元格样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) foreground: u32,
    pub(crate) background: u32,
    pub(crate) flags: u8,
}

impl Style {
    /// 默认样式。
    pub(crate) const DEFAULT: Self = Self {
        foreground: FOREGROUND,
        background: BACKGROUND,
        flags: 0,
    };

    /// 应用一组 SGR 参数。
    pub(crate) fn apply(&mut self, params: &[u16]) {
        if params.is_empty() {
            *self = Self::DEFAULT;
            return;
        }
        let mut index = 0;
        while index < params.len() {
            let code = params[index];
            index += 1;
            match code {
                0 => *self = Self::DEFAULT,
                1 => self.flags |= FLAG_BOLD,
                2 => self.flags |= FLAG_DIM,
                4 => self.flags |= FLAG_UNDERLINE,
                7 => self.flags |= FLAG_REVERSE,
                22 => self.flags &= !(FLAG_BOLD | FLAG_DIM),
                24 => self.flags &= !FLAG_UNDERLINE,
                27 => self.flags &= !FLAG_REVERSE,
                30..=37 => self.foreground = PALETTE[usize::from(code - 30)],
                39 => self.foreground = FOREGROUND,
                40..=47 => self.background = PALETTE[usize::from(code - 40)],
                49 => self.background = BACKGROUND,
                90..=97 => self.foreground = PALETTE[usize::from(code - 90) + 8],
                100..=107 => self.background = PALETTE[usize::from(code - 100) + 8],
                38 | 48 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        if code == 38 {
                            self.foreground = color;
                        } else {
                            self.background = color;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// 一个字符单元格。
#[derive(Clone, Copy)]
pub(crate) struct Cell {
    pub(crate) character: char,
    pub(crate) style: Style,
}

impl Cell {
    /// 默认空白单元格。
    pub(crate) const BLANK: Self = Self {
        character: ' ',
        style: Style::DEFAULT,
    };
}

fn extended_color(params: &[u16], index: &mut usize) -> Option<u32> {
    let mode = *params.get(*index)?;
    *index += 1;
    match mode {
        5 => {
            let value = *params.get(*index)? as u8;
            *index += 1;
            Some(palette_256(value))
        }
        2 => {
            let red = *params.get(*index)? as u8;
            let green = *params.get(*index + 1)? as u8;
            let blue = *params.get(*index + 2)? as u8;
            *index += 3;
            Some((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue))
        }
        _ => None,
    }
}

fn palette_256(value: u8) -> u32 {
    if value < 16 {
        return PALETTE[usize::from(value)];
    }
    if value < 232 {
        let index = u32::from(value - 16);
        let levels = [0_u32, 95, 135, 175, 215, 255];
        let red = levels[(index / 36) as usize];
        let green = levels[((index % 36) / 6) as usize];
        let blue = levels[(index % 6) as usize];
        return (red << 16) | (green << 8) | blue;
    }
    let level = 8 + u32::from(value - 232) * 10;
    (level << 16) | (level << 8) | level
}
