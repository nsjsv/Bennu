//! 文件大小的写法和读法：全软件一张单位表，1024 进制、IEC 单位（KDE `KFormat` 的默认）。
//! 显示、输入框回显、输入解析都从这里出，单位不会各写各的。

use crate::tr;

pub const BYTES_PER_KIB: u64 = 1024;

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
/// 宽度受限处的单字母缩写，下标与 `UNITS` 一一对应。
const COMPACT_UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];

/// 全应用唯一的文件大小写法：一位小数（"14.3 MiB"）；不到 1 KiB 写字节数（"123 B"）。
pub fn format_file_size(bytes: u64) -> String {
    format_size_with_units(bytes, UNITS, " ")
}

/// 宽度受限的短标签（分组索引栏）：数值口径与 [`format_file_size`] 相同，单位缩成
/// 一个字母、不留空格（"14.3M"）。
pub fn format_file_size_compact(bytes: u64) -> String {
    format_size_with_units(bytes, COMPACT_UNITS, "")
}

/// 输入框回显：取能整除的最大单位写整数（"8 MiB"、"1280 KiB"），用户不改再提交，
/// [`parse_file_size`] 读回来分毫不差；一位小数的 [`format_file_size`] 做不到。
pub fn format_file_size_exact(bytes: u64) -> String {
    let exponent = (1..UNITS.len())
        .rev()
        .find(|&exponent| bytes != 0 && bytes.is_multiple_of(unit_bytes(exponent)))
        .unwrap_or(0);
    format!("{} {}", bytes / unit_bytes(exponent), UNITS[exponent])
}

fn format_size_with_units(bytes: u64, units: [&str; 7], separator: &str) -> String {
    if bytes < BYTES_PER_KIB {
        return format!("{bytes}{separator}{}", units[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= BYTES_PER_KIB as f64 && unit < units.len() - 1 {
        value /= BYTES_PER_KIB as f64;
        unit += 1;
    }
    format!("{value:.1}{separator}{}", units[unit])
}

fn unit_bytes(exponent: usize) -> u64 {
    BYTES_PER_KIB.pow(exponent as u32)
}

/// 填的大小读不出来；只带用户原文，提示句由 [`FileSizeInputError::message`] 按界面语言给。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSizeInputError {
    UnknownUnit(String),
    InvalidValue(String),
    TooLarge(String),
}

impl FileSizeInputError {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownUnit(unit) => tr!("Unknown size unit: %1", unit),
            Self::InvalidValue(value) => tr!("Invalid size value: %1", value),
            Self::TooLarge(value) => tr!("Size value is too large: %1", value),
        }
    }
}

/// 读用户填的大小：非负十进制数（可带小数）+ 可选单位，数字和单位之间可留空格。
/// 单位不分大小写，`K`、`KB`、`KiB` 一样按 1024 算（大家习惯写 MB，显示的是 MiB，
/// 两种都得认），一直到 EiB；不写单位是字节。小数向下取整到字节。
pub fn parse_file_size(input: &str) -> Result<u64, FileSizeInputError> {
    let text = input.trim();
    let digits_end = text
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(text.len());
    let (number_text, unit_text) = text.split_at(digits_end);
    let exponent = unit_exponent(unit_text.trim())
        .ok_or_else(|| FileSizeInputError::UnknownUnit(unit_text.trim().to_owned()))?;
    parse_scaled_number(number_text.trim(), unit_bytes(exponent))
}

fn unit_exponent(unit: &str) -> Option<usize> {
    let unit = unit.to_lowercase();
    if matches!(unit.as_str(), "" | "b" | "byte" | "bytes" | "字节") {
        return Some(0);
    }
    let mut characters = unit.chars();
    let prefix = characters.next()?;
    let exponent = COMPACT_UNITS[1..]
        .iter()
        .position(|letter| letter.starts_with(prefix.to_ascii_uppercase()))?
        + 1;
    matches!(characters.as_str(), "" | "b" | "i" | "ib").then_some(exponent)
}

fn parse_scaled_number(number_text: &str, multiplier: u64) -> Result<u64, FileSizeInputError> {
    let invalid = || FileSizeInputError::InvalidValue(number_text.to_owned());
    let too_large = || FileSizeInputError::TooLarge(number_text.to_owned());
    let (integer_text, fractional_text) = number_text.split_once('.').unwrap_or((number_text, ""));
    // "1." 与 ".5" 这类残缺小数视为误输入，而不是静默补零。
    if integer_text.is_empty() || (number_text.contains('.') && fractional_text.is_empty()) {
        return Err(invalid());
    }
    let integer: u64 =
        integer_text
            .parse()
            .map_err(|error: std::num::ParseIntError| match error.kind() {
                std::num::IntErrorKind::PosOverflow => too_large(),
                _ => invalid(),
            })?;
    let bytes = u128::from(integer.checked_mul(multiplier).ok_or_else(too_large)?);
    if fractional_text.is_empty() {
        return u64::try_from(bytes).map_err(|_| too_large());
    }
    // 小数部分用 u128 中转，避免先乘单位再除时的中间溢出；超长小数乘大单位仍可能溢出
    // u128，和整数部分一样按太大拒绝，不能 panic 或回绕成错的字节数。
    let fractional: u128 = fractional_text.parse().map_err(|_| invalid())?;
    let scale = 10u128
        .checked_pow(fractional_text.len() as u32)
        .ok_or_else(too_large)?;
    let fractional_bytes = fractional
        .checked_mul(u128::from(multiplier))
        .ok_or_else(too_large)?
        / scale;
    u64::try_from(bytes + fractional_bytes).map_err(|_| too_large())
}

#[cfg(test)]
#[path = "file_size_tests.rs"]
mod tests;
