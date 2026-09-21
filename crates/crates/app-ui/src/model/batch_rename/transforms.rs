use std::path::Path;

use regex::Regex;

use super::{
    BatchRenameCaseRule, BatchRenameExtensionMode, BatchRenameInsertMode, BatchRenameRandomMode,
    BatchRenameRemoveClass, BatchRenameRemoveMode, BatchRenameReplaceScope, BatchRenameRuleParams,
    BatchRenameSliceMode, BatchRenameSource, BatchRenameState, BatchRenameTemplateToken,
};

/// 每轮预览只做一次的准备工作：正则编译与列表名单解析按规则 id 缓存。
pub(super) struct PreparedBatchRenameRules {
    regexes: Vec<(u64, Result<Regex, String>)>,
    list_names: Vec<(u64, Vec<String>)>,
}

impl PreparedBatchRenameRules {
    pub(super) fn new(state: &BatchRenameState) -> Self {
        let mut regexes = Vec::new();
        let mut list_names = Vec::new();
        for rule in &state.rules {
            match &rule.params {
                BatchRenameRuleParams::Regex(params) if !params.pattern.trim().is_empty() => {
                    let compiled = Regex::new(&params.pattern)
                        .map_err(|error| format!("invalid regex: {error}"));
                    regexes.push((rule.id, compiled));
                }
                BatchRenameRuleParams::List(params) => {
                    list_names.push((rule.id, parse_list_names(&params.names)));
                }
                _ => {}
            }
        }
        Self {
            regexes,
            list_names,
        }
    }

    fn regex_for(&self, id: u64) -> Option<&Result<Regex, String>> {
        self.regexes
            .iter()
            .find(|(rule_id, _)| *rule_id == id)
            .map(|(_, compiled)| compiled)
    }

    fn list_for(&self, id: u64) -> Option<&[String]> {
        self.list_names
            .iter()
            .find(|(rule_id, _)| *rule_id == id)
            .map(|(_, names)| names.as_slice())
    }
}

/// 规则管道求值：按用户排列的顺序逐条应用，每条规则以上一条的输出为输入。
pub(super) fn rename_with_rules(
    prepared: &PreparedBatchRenameRules,
    state: &BatchRenameState,
    item: &BatchRenameSource,
    preview_index: usize,
) -> Result<String, String> {
    let mut name = item.source_name_text.clone();
    for rule in state.rules.iter().filter(|rule| rule.enabled) {
        name = apply_rule(prepared, rule.id, &rule.params, name, item, preview_index)?;
    }
    Ok(name)
}

fn apply_rule(
    prepared: &PreparedBatchRenameRules,
    rule_id: u64,
    params: &BatchRenameRuleParams,
    name: String,
    item: &BatchRenameSource,
    preview_index: usize,
) -> Result<String, String> {
    match params {
        BatchRenameRuleParams::Template(params) => {
            let canonical = canonical_template(&params.template);
            let random_text = deterministic_random_text(
                &item.source_name_text,
                preview_index,
                6,
                "abcdefghijklmnopqrstuvwxyz0123456789",
            );
            Ok(render_custom_template(
                &canonical,
                &name,
                item,
                preview_index.saturating_add(1),
                0,
                &random_text,
            ))
        }
        BatchRenameRuleParams::Replace(params) => {
            if params.find.is_empty() {
                return Ok(name);
            }
            let (stem, extension) = split_file_name(&name);
            let stem = replace_text(
                &stem,
                &params.find,
                &params.replacement,
                params.scope,
                parse_optional_usize(&params.range_start_input),
                parse_optional_usize(&params.range_length_input),
                params.ignore_case,
            );
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Insert(params) => {
            if params.text.is_empty() {
                return Ok(name);
            }
            let position = parse_usize_or_default(&params.position_input, 0);
            if params.ignore_extension {
                let (stem, extension) = split_file_name(&name);
                let stem = insert_text(&stem, params, position);
                Ok(join_stem_extension(&stem, extension.as_deref()))
            } else {
                Ok(insert_text(&name, params, position))
            }
        }
        BatchRenameRuleParams::Slice(params) => {
            let start = parse_optional_usize(&params.start_input);
            let length = parse_optional_usize(&params.length_input);
            if start.is_none() && length.is_none() {
                return Ok(name);
            }
            let (stem, extension) = split_file_name(&name);
            let stem = slice_text(&stem, params, start.unwrap_or(0), length);
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Remove(params) => {
            let start = parse_optional_usize(&params.start_input);
            let length = parse_optional_usize(&params.length_input);
            let (stem, extension) = split_file_name(&name);
            let stem = remove_text(&stem, params, start.unwrap_or(0), length);
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Case(case) => {
            let (stem, extension) = split_file_name(&name);
            let stem = apply_case_rule(&stem, *case);
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Random(params) => {
            let random_text = deterministic_random_text(
                &item.source_name_text,
                preview_index,
                parse_usize_or_default(&params.length_input, 6).min(64),
                &params.alphabet,
            );
            let (stem, extension) = split_file_name(&name);
            let stem = apply_random_rule(stem, &random_text, params.mode);
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Sequence(params) => {
            let start = parse_usize_or_default(&params.start_input, 1);
            let step = parse_usize_or_default(&params.step_input, 1);
            let padding = parse_usize_or_default(&params.padding_input, 0);
            let sequence_number = start.saturating_add(preview_index.saturating_mul(step));
            let (stem, extension) = split_file_name(&name);
            let stem = apply_sequence_rule(stem, sequence_number, padding, params);
            let extension = params.preserve_extension.then_some(extension).flatten();
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Extension(params) => {
            let (stem, extension) = split_file_name(&name);
            let extension = apply_extension_rule(extension, params);
            Ok(join_stem_extension(&stem, extension.as_deref()))
        }
        BatchRenameRuleParams::Regex(params) => {
            let compiled = prepared
                .regex_for(rule_id)
                .ok_or_else(|| "regex pattern is empty".to_owned())?;
            let regex = compiled.clone()?;
            Ok(regex
                .replace_all(&name, params.replacement.as_str())
                .to_string())
        }
        BatchRenameRuleParams::List(params) => {
            if params.names.trim().is_empty() {
                return Ok(name);
            }
            let names = prepared
                .list_for(rule_id)
                .ok_or_else(|| "list rule is missing".to_owned())?;
            Ok(names
                .get(preview_index)
                .cloned()
                .unwrap_or_else(|| name.clone()))
        }
    }
}

fn apply_sequence_rule(
    stem: String,
    sequence_number: usize,
    padding: usize,
    params: &super::BatchRenameSequenceRule,
) -> String {
    if params.prefix.is_empty() && params.include_original_stem {
        return stem;
    }

    let number = padded_number(sequence_number, padding);
    let mut name = format!("{}{}", params.prefix, number);
    if params.include_original_stem && !stem.is_empty() {
        name.push(' ');
        name.push_str(&stem);
    }
    name
}

fn replace_text(
    source: &str,
    find: &str,
    replacement: &str,
    scope: BatchRenameReplaceScope,
    range_start: Option<usize>,
    range_length: Option<usize>,
    ignore_case: bool,
) -> String {
    match scope {
        BatchRenameReplaceScope::All => replace_all(source, find, replacement, ignore_case),
        BatchRenameReplaceScope::First => {
            replace_first_match(source, find, replacement, ignore_case)
                .unwrap_or_else(|| source.to_owned())
        }
        BatchRenameReplaceScope::Last => replace_last_match(source, find, replacement, ignore_case)
            .unwrap_or_else(|| source.to_owned()),
        BatchRenameReplaceScope::Range => replace_in_range(
            source,
            find,
            replacement,
            range_start.unwrap_or(0),
            range_length,
            ignore_case,
        ),
    }
}

fn insert_text(source: &str, params: &super::BatchRenameInsertRule, position: usize) -> String {
    match params.mode {
        BatchRenameInsertMode::Before => format!("{}{}", params.text, source),
        BatchRenameInsertMode::After => format!("{}{}", source, params.text),
        BatchRenameInsertMode::Position => {
            insert_text_at_char_position(source, &params.text, position)
        }
        BatchRenameInsertMode::AfterAnchor => {
            insert_text_after_anchor(source, &params.anchor, &params.text)
        }
    }
}

fn slice_text(
    source: &str,
    params: &super::BatchRenameSliceRule,
    start: usize,
    length: Option<usize>,
) -> String {
    match params.mode {
        BatchRenameSliceMode::Position => slice_text_by_chars(source, start, length),
        BatchRenameSliceMode::AfterAnchor => {
            slice_text_after_anchor(source, &params.anchor, length)
        }
    }
}

fn remove_text(
    source: &str,
    params: &super::BatchRenameRemoveRule,
    start: usize,
    length: Option<usize>,
) -> String {
    match params.mode {
        BatchRenameRemoveMode::TextAndRange => {
            let mut next = source.to_owned();
            if !params.text.is_empty() {
                next = next.replace(&params.text, "");
            }
            if !params.start_input.trim().is_empty() || !params.length_input.trim().is_empty() {
                next = remove_text_by_char_range(&next, start, length);
            }
            next
        }
        BatchRenameRemoveMode::CharacterClasses => remove_char_classes(source, &params.classes),
    }
}

fn apply_extension_rule(
    extension: Option<String>,
    params: &super::BatchRenameExtensionRule,
) -> Option<String> {
    match params.mode {
        BatchRenameExtensionMode::Preserve => extension,
        BatchRenameExtensionMode::Remove => None,
        BatchRenameExtensionMode::Replace => normalize_extension(&params.replacement),
        BatchRenameExtensionMode::Lowercase => extension.map(|extension| extension.to_lowercase()),
        BatchRenameExtensionMode::Uppercase => extension.map(|extension| extension.to_uppercase()),
    }
}

fn apply_random_rule(stem: String, random_text: &str, mode: BatchRenameRandomMode) -> String {
    match mode {
        BatchRenameRandomMode::Off => stem,
        BatchRenameRandomMode::ReplaceStem => random_text.to_owned(),
        BatchRenameRandomMode::Prefix => format!("{random_text}{stem}"),
        BatchRenameRandomMode::Suffix => format!("{stem}{random_text}"),
    }
}

fn canonical_template(template: &str) -> String {
    let mut canonical = template.to_owned();
    for token in BatchRenameTemplateToken::ALL {
        for label in token.localized_labels() {
            canonical = canonical.replace(&label, token.engine_token());
        }
    }
    canonical
}

fn render_custom_template(
    template: &str,
    current_name: &str,
    item: &BatchRenameSource,
    sequence_number: usize,
    padding: usize,
    random_text: &str,
) -> String {
    let (current_stem, current_extension) = split_file_name(current_name);
    let (original_stem, original_extension) = split_file_name(&item.source_name_text);
    template
        .replace("{name}", current_name)
        .replace("{stem}", &current_stem)
        .replace("{ext}", current_extension.as_deref().unwrap_or(""))
        .replace("{index}", &sequence_number.to_string())
        .replace("{n}", &padded_number(sequence_number, padding))
        .replace("{n2}", &padded_number(sequence_number, 2))
        .replace("{n3}", &padded_number(sequence_number, 3))
        .replace("{original}", &item.source_name_text)
        .replace("{original_stem}", &original_stem)
        .replace(
            "{original_ext}",
            original_extension.as_deref().unwrap_or(""),
        )
        .replace("{random}", random_text)
}

fn parse_list_names(input: &str) -> Vec<String> {
    if input.trim().is_empty() {
        return Vec::new();
    }

    let names = if input.contains('\n') {
        input.lines().collect::<Vec<_>>()
    } else {
        input.split('|').collect::<Vec<_>>()
    };
    names
        .into_iter()
        .map(|name| name.trim_matches('\r').trim().to_owned())
        .collect()
}

fn normalize_extension(input: &str) -> Option<String> {
    let extension = input.trim().trim_start_matches('.');
    if extension.is_empty() || extension.eq_ignore_ascii_case("remove") {
        None
    } else {
        Some(extension.to_owned())
    }
}

fn split_file_name(name: &str) -> (String, Option<String>) {
    let path = Path::new(name);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(ToOwned::to_owned);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .to_owned();
    (stem, extension)
}

fn join_stem_extension(stem: &str, extension: Option<&str>) -> String {
    match extension {
        Some(extension) if !extension.is_empty() => format!("{stem}.{extension}"),
        _ => stem.to_owned(),
    }
}

fn insert_text_at_char_position(source: &str, text: &str, position: usize) -> String {
    let byte_position = source
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(source.len()))
        .nth(position)
        .unwrap_or(source.len());
    let mut next = String::with_capacity(source.len() + text.len());
    next.push_str(&source[..byte_position]);
    next.push_str(text);
    next.push_str(&source[byte_position..]);
    next
}

fn insert_text_after_anchor(source: &str, anchor: &str, text: &str) -> String {
    if anchor.is_empty() {
        return format!("{source}{text}");
    }
    let Some(byte_position) = source.find(anchor).map(|index| index + anchor.len()) else {
        return format!("{source}{text}");
    };
    let mut next = String::with_capacity(source.len() + text.len());
    next.push_str(&source[..byte_position]);
    next.push_str(text);
    next.push_str(&source[byte_position..]);
    next
}

fn slice_text_by_chars(source: &str, start: usize, length: Option<usize>) -> String {
    let chars = source.chars().collect::<Vec<_>>();
    if start >= chars.len() {
        return String::new();
    }
    let end = length
        .map(|length| start.saturating_add(length).min(chars.len()))
        .unwrap_or(chars.len());
    chars[start..end].iter().collect()
}

fn slice_text_after_anchor(source: &str, anchor: &str, length: Option<usize>) -> String {
    if anchor.is_empty() {
        return String::new();
    }
    let Some(start) = source
        .find(anchor)
        .map(|index| source[..index + anchor.len()].chars().count())
    else {
        return String::new();
    };
    slice_text_by_chars(source, start, length)
}

fn remove_text_by_char_range(source: &str, start: usize, length: Option<usize>) -> String {
    let chars = source.chars().collect::<Vec<_>>();
    if start >= chars.len() {
        return source.to_owned();
    }
    let end = length
        .map(|length| start.saturating_add(length).min(chars.len()))
        .unwrap_or(chars.len());
    chars
        .iter()
        .enumerate()
        .filter_map(|(index, character)| {
            if (start..end).contains(&index) {
                None
            } else {
                Some(*character)
            }
        })
        .collect()
}

fn remove_char_classes(source: &str, classes: &[BatchRenameRemoveClass]) -> String {
    source
        .chars()
        .filter(|character| {
            !classes
                .iter()
                .any(|class| remove_class_matches(*class, *character))
        })
        .collect()
}

fn remove_class_matches(class: BatchRenameRemoveClass, character: char) -> bool {
    match class {
        BatchRenameRemoveClass::Lowercase => character.is_lowercase(),
        BatchRenameRemoveClass::Uppercase => character.is_uppercase(),
        BatchRenameRemoveClass::Digits => character.is_ascii_digit(),
        BatchRenameRemoveClass::Symbols => {
            !character.is_alphanumeric() && !character.is_whitespace() && !is_bracket(character)
        }
        BatchRenameRemoveClass::Brackets => is_bracket(character),
        BatchRenameRemoveClass::Whitespace => character.is_whitespace(),
        BatchRenameRemoveClass::Hanzi => is_hanzi(character),
    }
}

fn is_bracket(character: char) -> bool {
    matches!(character, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>')
}

fn is_hanzi(character: char) -> bool {
    matches!(character as u32,
        0x3400..=0x4DBF |
        0x4E00..=0x9FFF |
        0xF900..=0xFAFF |
        0x20000..=0x2A6DF |
        0x2A700..=0x2B73F |
        0x2B740..=0x2B81F |
        0x2B820..=0x2CEAF |
        0x2CEB0..=0x2EBEF |
        0x30000..=0x3134F)
}

fn replace_all(source: &str, find: &str, replacement: &str, ignore_case: bool) -> String {
    if !ignore_case {
        return source.replace(find, replacement);
    }
    replace_all_case_insensitive(source, find, replacement)
}

fn replace_all_case_insensitive(source: &str, find: &str, replacement: &str) -> String {
    let mut next = String::new();
    let mut remainder = source;
    while let Some((start, end)) = find_case_insensitive(remainder, find) {
        next.push_str(&remainder[..start]);
        next.push_str(replacement);
        remainder = &remainder[end..];
    }
    next.push_str(remainder);
    next
}

fn replace_first_match(
    source: &str,
    find: &str,
    replacement: &str,
    ignore_case: bool,
) -> Option<String> {
    let (start, end) = if ignore_case {
        find_case_insensitive(source, find)?
    } else {
        let start = source.find(find)?;
        (start, start + find.len())
    };
    Some(replace_byte_range(source, start, end, replacement))
}

fn replace_last_match(
    source: &str,
    find: &str,
    replacement: &str,
    ignore_case: bool,
) -> Option<String> {
    let (start, end) = if ignore_case {
        find_last_case_insensitive(source, find)?
    } else {
        let start = source.rfind(find)?;
        (start, start + find.len())
    };
    Some(replace_byte_range(source, start, end, replacement))
}

fn replace_in_range(
    source: &str,
    find: &str,
    replacement: &str,
    start: usize,
    length: Option<usize>,
    ignore_case: bool,
) -> String {
    let chars = source.chars().collect::<Vec<_>>();
    if start >= chars.len() {
        return source.to_owned();
    }
    let end = length
        .map(|value| start.saturating_add(value).min(chars.len()))
        .unwrap_or(chars.len());
    let prefix: String = chars[..start].iter().collect();
    let middle: String = chars[start..end].iter().collect();
    let suffix: String = chars[end..].iter().collect();
    format!(
        "{}{}{}",
        prefix,
        replace_all(&middle, find, replacement, ignore_case),
        suffix
    )
}

fn replace_byte_range(source: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut next = String::with_capacity(source.len() + replacement.len());
    next.push_str(&source[..start]);
    next.push_str(replacement);
    next.push_str(&source[end..]);
    next
}

fn find_case_insensitive(source: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    for (start, _) in source.char_indices() {
        let end = source[start..]
            .char_indices()
            .nth(needle.chars().count())
            .map(|(offset, _)| start + offset)
            .unwrap_or(source.len());
        if case_insensitive_match(&source[start..end], needle) {
            return Some((start, end));
        }
    }
    None
}

fn find_last_case_insensitive(source: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let mut last = None;
    for (start, _) in source.char_indices() {
        let end = source[start..]
            .char_indices()
            .nth(needle.chars().count())
            .map(|(offset, _)| start + offset)
            .unwrap_or(source.len());
        if case_insensitive_match(&source[start..end], needle) {
            last = Some((start, end));
        }
    }
    last
}

fn case_insensitive_match(source: &str, needle: &str) -> bool {
    source
        .chars()
        .flat_map(|character| character.to_lowercase())
        .eq(needle
            .chars()
            .flat_map(|character| character.to_lowercase()))
}

fn apply_case_rule(source: &str, case: BatchRenameCaseRule) -> String {
    match case {
        BatchRenameCaseRule::Unchanged => source.to_owned(),
        BatchRenameCaseRule::Lowercase => source.to_lowercase(),
        BatchRenameCaseRule::Uppercase => source.to_uppercase(),
        BatchRenameCaseRule::TitleCase => title_case(source),
        BatchRenameCaseRule::InvertCase => invert_case(source),
    }
}

fn title_case(source: &str) -> String {
    let mut next_word = true;
    let mut output = String::new();
    for character in source.chars() {
        if character.is_alphanumeric() {
            if next_word {
                output.extend(character.to_uppercase());
                next_word = false;
            } else {
                output.extend(character.to_lowercase());
            }
        } else {
            output.push(character);
            next_word = true;
        }
    }
    output
}

fn invert_case(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    for character in source.chars() {
        if character.is_lowercase() {
            output.extend(character.to_uppercase());
        } else if character.is_uppercase() {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn deterministic_random_text(name: &str, index: usize, length: usize, alphabet: &str) -> String {
    let alphabet = alphabet.chars().collect::<Vec<_>>();
    let alphabet = if alphabet.is_empty() {
        "abcdefghijklmnopqrstuvwxyz0123456789"
            .chars()
            .collect::<Vec<_>>()
    } else {
        alphabet
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(name.as_bytes());
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    let mut seed_bytes = [0u8; 8];
    seed_bytes.copy_from_slice(&hash.as_bytes()[..8]);
    let mut seed = u64::from_le_bytes(seed_bytes);

    let mut output = String::new();
    for _ in 0..length {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let character = alphabet[(seed as usize) % alphabet.len()];
        output.push(character);
    }
    output
}

fn padded_number(number: usize, padding: usize) -> String {
    if padding == 0 {
        number.to_string()
    } else {
        format!("{number:0padding$}")
    }
}

fn parse_usize_or_default(input: &str, default: usize) -> usize {
    input.trim().parse::<usize>().unwrap_or(default)
}

fn parse_optional_usize(input: &str) -> Option<usize> {
    let input = input.trim();
    if input.is_empty() {
        None
    } else {
        input.parse::<usize>().ok()
    }
}
