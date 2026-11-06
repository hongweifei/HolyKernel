//! 文档指令解析与文本度量。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_canvas::GlyphSource;

use crate::document::{DocSet, Run, Style};

/// 返回字符在该字体下的前进宽度。
pub(crate) fn char_width(font: &impl GlyphSource, character: char) -> usize {
    font.advance(character).max(0) as usize
}

/// 从 `@title(...)` 提取页面标题。
pub(crate) fn title_of(body: &str) -> Option<String> {
    for line in body.lines() {
        if let Some((name, rest)) = call(line.trim())
            && name == "title"
            && let Some(title) = split_args(rest).first()
        {
            return Some(title.clone());
        }
    }
    None
}

/// DocPath 的最后一段，作为缺省标题。
pub(crate) fn doc_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// 拆分 `@指令` 名称与参数文本。
pub(crate) fn call(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('@')?;
    let end = rest
        .find(|character: char| character == '(' || character.is_whitespace())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some((&rest[..end], rest[end..].trim()))
}

/// 拆分逗号分隔的参数，保留引号内的逗号，去掉成对引号。
pub(crate) fn split_args(text: &str) -> Vec<String> {
    let text = text.trim();
    let text = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(text);
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() || !args.is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

/// 解析行内链接写法 `[label](target)`。
pub(crate) fn parse_inline(set: &DocSet, text: &str, style: Style) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find("](") else {
            break;
        };
        let close = open + close_rel;
        let Some(end_rel) = rest[close + 2..].find(')') else {
            break;
        };
        let end = close + 2 + end_rel;
        if open > 0 {
            runs.push(Run {
                text: String::from(&rest[..open]),
                style,
                target: None,
            });
        }
        let label = &rest[open + 1..close];
        let target = set.resolve(&rest[close + 2..end]);
        runs.push(Run {
            text: String::from(label),
            style: Style::Link,
            target,
        });
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        runs.push(Run {
            text: String::from(rest),
            style,
            target: None,
        });
    }
    runs
}
