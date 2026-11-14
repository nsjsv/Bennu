//! `fnmatch(pattern, name, 0)`：类型库里少数后缀树表达不了的名字规则用它比对
//! （`README*`、`*.[1-9]`、`makefile.*`）。不带标志，所以 `*` 也匹配 `/` 和开头的点。

pub(super) fn fnmatch(pattern: &str, name: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let name = name.chars().collect::<Vec<_>>();
    matches_from(&pattern, &name)
}

/// 回溯到最近一个 `*`：`*` 先吞零个字，后面对不上再让它多吞一个。
fn matches_from(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
                continue;
            }
            Some('?') => {
                p += 1;
                n += 1;
                continue;
            }
            Some('[') => {
                if let Some((matched, next)) = bracket(pattern, p, name[n]) {
                    if matched {
                        p = next;
                        n += 1;
                        continue;
                    }
                } else if name[n] == '[' {
                    p += 1;
                    n += 1;
                    continue;
                }
            }
            Some('\\') if pattern.get(p + 1) == Some(&name[n]) => {
                p += 2;
                n += 1;
                continue;
            }
            Some(literal) if *literal == name[n] => {
                p += 1;
                n += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((star_p, star_n)) => {
                p = star_p + 1;
                n = star_n + 1;
                star = Some((star_p, star_n + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

/// `[...]` 字符类：`!` 或 `^` 取反，`a-z` 区间，`]` 放第一个时是普通字符。
/// 返回（是否命中，类结束后的位置）；没有闭合的 `]` 时返回 None，`[` 当普通字符。
fn bracket(pattern: &[char], open: usize, character: char) -> Option<(bool, usize)> {
    let mut index = open + 1;
    let negated = matches!(pattern.get(index), Some('!' | '^'));
    if negated {
        index += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let current = *pattern.get(index)?;
        if current == ']' && !first {
            return Some((matched != negated, index + 1));
        }
        first = false;
        if pattern.get(index + 1) == Some(&'-')
            && pattern.get(index + 2).is_some_and(|end| *end != ']')
        {
            let end = pattern[index + 2];
            matched |= (current..=end).contains(&character);
            index += 3;
        } else {
            matched |= current == character;
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fnmatch;

    #[test]
    fn wildcards_and_classes() {
        assert!(fnmatch("README*", "README.md"));
        assert!(fnmatch("README*", "README"));
        assert!(!fnmatch("README*", "XREADME"));
        assert!(fnmatch("*.[1-9]", "ls.1"));
        assert!(!fnmatch("*.[1-9]", "ls.0"));
        assert!(fnmatch("makefile.*", "makefile.am"));
        assert!(fnmatch("*.?pp", "main.cpp"));
        assert!(fnmatch("[!a]*", "bcd"));
        assert!(!fnmatch("[!a]*", "abc"));
        assert!(fnmatch("*a*b*", "xxaxxbxx"));
        assert!(!fnmatch("*a*b", "xxaxxbxx"));
        assert!(fnmatch("[]x]", "]"));
        assert!(fnmatch("a[", "a["));
        assert!(fnmatch("*", ""));
        assert!(fnmatch("文档*", "文档一.txt"));
    }
}
