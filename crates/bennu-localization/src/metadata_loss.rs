use crate::UiLanguage;

/// 复制后没能保留的元数据种类。本 crate 不依赖 file-core,由调用方映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAttributeKind {
    Permissions,
    ExtendedAttributes,
    AccessControlList,
}

/// 完成提示里的一条明细:文件名与没能保留的种类。
pub struct LostAttributesText {
    pub name: String,
    pub kinds: Vec<LostAttributeKind>,
}

/// 构造时按给定语言产出成品。文件名是用户数据,不能交给按字符串拆分的
/// 翻译表去还原,所以整句在这里一次生成。`remaining` 是明细之外的条目数。
pub(super) fn warning(
    language: UiLanguage,
    details: &[LostAttributesText],
    remaining: u64,
) -> String {
    match language {
        UiLanguage::English => {
            let mut items = details
                .iter()
                .map(|detail| {
                    let kinds = detail
                        .kinds
                        .iter()
                        .map(|kind| english_kind(*kind))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{} ({kinds})", detail.name)
                })
                .collect::<Vec<_>>();
            if remaining > 0 {
                items.push(format!("... (+{remaining})"));
            }
            format!(
                "Some file attributes could not be preserved: {}",
                items.join("; ")
            )
        }
        UiLanguage::Chinese => {
            let mut items = details
                .iter()
                .map(|detail| {
                    let kinds = detail
                        .kinds
                        .iter()
                        .map(|kind| chinese_kind(*kind))
                        .collect::<Vec<_>>()
                        .join("、");
                    format!("{}（{kinds}）", detail.name)
                })
                .collect::<Vec<_>>();
            if remaining > 0 {
                items.push(format!("……（另 {remaining} 个）"));
            }
            format!("部分文件属性未能保留：{}", items.join("；"))
        }
    }
}

fn english_kind(kind: LostAttributeKind) -> &'static str {
    match kind {
        LostAttributeKind::Permissions => "permissions",
        LostAttributeKind::ExtendedAttributes => "extended attributes",
        LostAttributeKind::AccessControlList => "access control list",
    }
}

fn chinese_kind(kind: LostAttributeKind) -> &'static str {
    match kind {
        LostAttributeKind::Permissions => "权限",
        LostAttributeKind::ExtendedAttributes => "扩展属性",
        LostAttributeKind::AccessControlList => "访问控制列表",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details() -> Vec<LostAttributesText> {
        vec![
            LostAttributesText {
                name: "a; b (1).txt".to_owned(),
                kinds: vec![
                    LostAttributeKind::Permissions,
                    LostAttributeKind::ExtendedAttributes,
                ],
            },
            LostAttributesText {
                name: "photos".to_owned(),
                kinds: vec![LostAttributeKind::AccessControlList],
            },
        ]
    }

    #[test]
    fn english_warning_lists_names_kinds_and_remaining_count() {
        assert_eq!(
            warning(UiLanguage::English, &details(), 3),
            "Some file attributes could not be preserved: \
             a; b (1).txt (permissions, extended attributes); \
             photos (access control list); ... (+3)"
        );
    }

    #[test]
    fn chinese_warning_lists_names_kinds_and_remaining_count() {
        assert_eq!(
            warning(UiLanguage::Chinese, &details(), 3),
            "部分文件属性未能保留：a; b (1).txt（权限、扩展属性）；photos（访问控制列表）；……（另 3 个）"
        );
    }

    #[test]
    fn remaining_marker_is_omitted_when_every_entry_is_listed() {
        assert!(!warning(UiLanguage::English, &details(), 0).contains("..."));
    }
}
