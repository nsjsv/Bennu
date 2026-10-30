//! 消息 ID：英文句式本身就是 ID（同 KDE `i18n("Move to %1", name)`、GNOME gettext）。
//! `tr!` 在编译时查表：找不到这句、或参数个数和 `%N` 对不上，编译就失败。运行时按
//! 下标取当前语言的句式再填参数；参数只被填进去、从不参与查找，所以文件名之类的
//! 用户数据不会被当成文案翻译。

use std::fmt::Display;

use crate::catalog::TABLES;
use crate::UiLanguage;

/// 文案表里一条句式的位置。只由 `tr!` / `tr_in!` 在 const 里构造。
#[derive(Debug, Clone, Copy)]
pub struct MessageId {
    table: usize,
    entry: usize,
}

impl MessageId {
    /// 在 const 里求值：panic 就是编译错误，报在调用 `tr!` 的那一行。
    pub const fn of(english: &str, arguments: usize) -> Self {
        let mut table = 0;
        while table < TABLES.len() {
            let mut entry = 0;
            while entry < TABLES[table].len() {
                if same_text(TABLES[table][entry].0, english) {
                    assert!(
                        highest_placeholder(english) == arguments,
                        "tr!: the number of arguments does not match the %N placeholders"
                    );
                    return Self { table, entry };
                }
                entry += 1;
            }
            table += 1;
        }
        panic!("tr!: this text has no entry in the bennu-localization catalog")
    }

    pub fn format(self, language: UiLanguage, arguments: &[&dyn Display]) -> String {
        let (english, chinese) = TABLES[self.table][self.entry];
        let template = match language {
            UiLanguage::English => english,
            UiLanguage::Chinese => chinese,
        };
        fill_placeholders(template, arguments)
    }
}

/// 当前界面语言的文案：`tr!("Copy to %1", name)`。
#[macro_export]
macro_rules! tr {
    ($english:literal $(, $argument:expr)* $(,)?) => {
        $crate::tr_in!($crate::current_language(), $english $(, $argument)*)
    };
}

/// 指定语言的文案，给结果要在测试里按中英文分别检查的函数用。
#[macro_export]
macro_rules! tr_in {
    ($language:expr, $english:literal $(, $argument:expr)* $(,)?) => {{
        const ID: $crate::MessageId =
            $crate::MessageId::of($english, <[&str]>::len(&[$(stringify!($argument)),*]));
        ID.format($language, &[$(&$argument as &dyn ::core::fmt::Display),*])
    }};
}

const fn same_text(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn highest_placeholder(template: &str) -> usize {
    let bytes = template.as_bytes();
    let mut highest = 0;
    let mut index = 0;
    while index + 1 < bytes.len() {
        if let Some(number) = placeholder_number(bytes[index], bytes[index + 1]) {
            if number > highest {
                highest = number;
            }
        }
        index += 1;
    }
    highest
}

/// `%1`…`%9` 是参数；别的 `%` 照原样显示（"Volume %1%"）。
const fn placeholder_number(percent: u8, digit: u8) -> Option<usize> {
    if percent == b'%' && digit >= b'1' && digit <= b'9' {
        Some((digit - b'0') as usize)
    } else {
        None
    }
}

fn fill_placeholders(template: &str, arguments: &[&dyn Display]) -> String {
    let mut text = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('%') {
        text.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after
            .bytes()
            .next()
            .and_then(|digit| placeholder_number(b'%', digit))
        {
            Some(number) => {
                text.push_str(&arguments[number - 1].to_string());
                rest = &after[1..];
            }
            None => {
                text.push('%');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
