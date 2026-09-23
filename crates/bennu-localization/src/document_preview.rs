pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "PDF preview requires Poppler. Install the poppler package." => {
            "PDF 预览需要 Poppler，请安装 poppler 软件包。"
        }
        "Encrypted PDF preview is not supported" => "不支持预览加密 PDF",
        "Office preview requires LibreOffice. Install the libreoffice package." => {
            "Office 预览需要 LibreOffice，请安装 libreoffice 软件包。"
        }
        "Office preview source is not a regular file" => "Office 预览源不是普通文件",
        "Office document converter is unavailable" => "Office 文档转换器不可用",
        "PDF preview requires at least one page" => "PDF 不包含可预览页面",
        "PDF preview supports at most 10000 pages" => "PDF 预览最多支持 10000 页",
        "PDF preview has an invalid page count" => "PDF 页数无效",
        "PDF preview source is not a regular file" => "PDF 预览源不是普通文件",
        "PDF page renderer is unavailable" => "PDF 页面渲染器不可用",
        "PDF page has invalid dimensions" => "PDF 页面尺寸无效",
        "PDF page exceeds the rendering budget" => "PDF 页面超过渲染资源上限",
        "PDF page exceeds the rendering pixel budget" => "PDF 页面超过渲染像素上限",
        "PDF page exceeds the preview memory budget" => "PDF 页面超过预览内存上限",
        "PDF page is too tall" => "PDF 页面过高，无法预览",
        "PDF page rendering size overflowed" => "PDF 页面渲染尺寸溢出",
        "PDF document layout height overflowed" => "PDF 文档布局高度溢出",
        "Rendering page..." => "正在渲染页面...",
        "Page deferred by preview memory limit" => "页面因预览内存上限暂缓渲染",
        _ => return translate_dynamic(text),
    };
    Some(exact.to_owned())
}

fn translate_dynamic(text: &str) -> Option<String> {
    if let Some(translated) = translate_office_validation_failure(text) {
        return Some(translated);
    }
    if let Some(translated) = translate_page_render_failure(text) {
        return Some(translated);
    }
    if let Some(translated) = translate_page_output_detail(text) {
        return Some(translated);
    }
    if let Some(translated) = translate_invalid_page_layout(text) {
        return Some(translated);
    }
    for (prefix, translated_prefix) in [
        ("Could not inspect PDF: ", "无法检查 PDF："),
        (
            "Could not inspect Office document: ",
            "无法检查 Office 文档：",
        ),
        (
            "Could not convert Office document: ",
            "无法转换 Office 文档：",
        ),
        (
            "Could not create PDF preview workspace: ",
            "无法创建 PDF 预览工作区：",
        ),
        (
            "Could not create Office preview workspace: ",
            "无法创建 Office 预览工作区：",
        ),
        (
            "PDF page metadata is incomplete for page ",
            "PDF 页面元数据不完整，页码：",
        ),
        (
            "PDF page has an invalid CropBox on page ",
            "PDF 页面 CropBox 无效，页码：",
        ),
        (
            "PDF page has an invalid rotation on page ",
            "PDF 页面旋转值无效，页码：",
        ),
        ("PDF page ", "PDF 页面 "),
    ] {
        if let Some(detail) = text.strip_prefix(prefix) {
            return Some(format!("{translated_prefix}{detail}"));
        }
    }
    text.starts_with("Poppler ")
        .then(|| format!("PDF 元数据无效：{text}"))
}

fn translate_page_render_failure(text: &str) -> Option<String> {
    let detail = text.strip_prefix("Could not render PDF page: ")?;
    let translated_detail =
        translate_page_output_detail(detail).unwrap_or_else(|| detail.to_owned());
    Some(format!("无法渲染 PDF 页面：{translated_detail}"))
}

fn translate_page_output_detail(text: &str) -> Option<String> {
    let exact = match text {
        "Poppler produced an empty PDF page image" => "Poppler 生成了空的 PDF 页面图像",
        "Rendered PDF page image exceeds the file budget" => {
            "已渲染的 PDF 页面图像超过文件大小上限"
        }
        "Rendered PDF page dimensions overflowed" => "已渲染的 PDF 页面像素尺寸溢出",
        "Rendered PDF page memory size overflowed" => "已渲染的 PDF 页面内存大小溢出",
        "Rendered PDF page exceeds the planned rendering budget" => {
            "已渲染的 PDF 页面超过计划渲染资源上限"
        }
        _ => {
            for (prefix, translated_prefix) in [
                (
                    "Could not inspect rendered PDF page: ",
                    "无法检查已渲染的 PDF 页面：",
                ),
                (
                    "Could not read rendered PDF page: ",
                    "无法读取已渲染的 PDF 页面：",
                ),
            ] {
                if let Some(diagnostic) = text.strip_prefix(prefix) {
                    return Some(format!("{translated_prefix}{diagnostic}"));
                }
            }
            return None;
        }
    };
    Some(exact.to_owned())
}

fn translate_invalid_page_layout(text: &str) -> Option<String> {
    let page_number = text
        .strip_prefix("PDF page ")?
        .strip_suffix(" has invalid layout dimensions")?
        .parse::<usize>()
        .ok()?;
    Some(format!("PDF 第 {page_number} 页布局尺寸无效"))
}

fn translate_office_validation_failure(text: &str) -> Option<String> {
    let detail = text.strip_prefix("Could not convert Office document: ")?;
    let translated_detail = match detail {
        "LibreOffice did not produce a PDF" => "LibreOffice 未生成 PDF".to_owned(),
        "LibreOffice produced more than one output" => "LibreOffice 生成了多个输出文件".to_owned(),
        "LibreOffice output is not a regular PDF file" => {
            "LibreOffice 输出不是非符号链接的普通 PDF 文件".to_owned()
        }
        "LibreOffice output is not a PDF file" => "LibreOffice 输出不是 PDF 文件".to_owned(),
        "LibreOffice produced an empty PDF" => "LibreOffice 生成了空 PDF".to_owned(),
        "LibreOffice output directory identity changed" => {
            "LibreOffice 输出目录身份已变化".to_owned()
        }
        "LibreOffice output escaped the preview workspace" => {
            "LibreOffice 输出逃离了预览工作区".to_owned()
        }
        _ => translate_office_inspection_failure(detail)
            .or_else(|| translate_oversized_office_output(detail))?,
    };
    Some(format!("无法转换 Office 文档：{translated_detail}"))
}

fn translate_office_inspection_failure(detail: &str) -> Option<String> {
    for (prefix, translated_prefix) in [
        (
            "Could not inspect LibreOffice output directory: ",
            "无法检查 LibreOffice 输出目录：",
        ),
        (
            "Could not inspect LibreOffice output: ",
            "无法检查 LibreOffice 输出：",
        ),
    ] {
        if let Some(os_error) = detail.strip_prefix(prefix) {
            return Some(format!("{translated_prefix}{os_error}"));
        }
    }
    None
}

fn translate_oversized_office_output(detail: &str) -> Option<String> {
    let detail = detail.strip_prefix("Converted Office PDF is too large to preview (")?;
    let detail = detail.strip_suffix('.')?;
    let (actual_size, maximum_size) = detail.split_once("). Maximum preview size is ")?;
    Some(format!(
        "转换后的 Office PDF 过大（{actual_size}），最大预览大小为 {maximum_size}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_document_tool_and_page_failures() {
        assert_eq!(
            translate("PDF preview requires Poppler. Install the poppler package.").unwrap(),
            "PDF 预览需要 Poppler，请安装 poppler 软件包。"
        );
        assert_eq!(
            translate("Could not inspect PDF: pdfinfo timed out").unwrap(),
            "无法检查 PDF：pdfinfo timed out"
        );
        assert_eq!(
            translate("Office preview requires LibreOffice. Install the libreoffice package.")
                .unwrap(),
            "Office 预览需要 LibreOffice，请安装 libreoffice 软件包。"
        );
        assert_eq!(
            translate("Could not convert Office document: libreoffice timed out").unwrap(),
            "无法转换 Office 文档：libreoffice timed out"
        );
        assert_eq!(
            translate("Could not render PDF page: pdftoppm failed").unwrap(),
            "无法渲染 PDF 页面：pdftoppm failed"
        );
        assert_eq!(
            translate("PDF document layout height overflowed").unwrap(),
            "PDF 文档布局高度溢出"
        );
        assert!(translate("Poppler omitted CropBox for PDF page 2")
            .unwrap()
            .starts_with("PDF 元数据无效："));
    }

    #[test]
    fn translates_every_application_owned_page_output_reason() {
        for (reason, expected) in [
            (
                "Poppler produced an empty PDF page image",
                "Poppler 生成了空的 PDF 页面图像",
            ),
            (
                "Rendered PDF page image exceeds the file budget",
                "已渲染的 PDF 页面图像超过文件大小上限",
            ),
            (
                "Rendered PDF page dimensions overflowed",
                "已渲染的 PDF 页面像素尺寸溢出",
            ),
            (
                "Rendered PDF page memory size overflowed",
                "已渲染的 PDF 页面内存大小溢出",
            ),
            (
                "Rendered PDF page exceeds the planned rendering budget",
                "已渲染的 PDF 页面超过计划渲染资源上限",
            ),
        ] {
            assert_eq!(translate(reason).unwrap(), expected);
            assert_eq!(
                translate(&format!("Could not render PDF page: {reason}")).unwrap(),
                format!("无法渲染 PDF 页面：{expected}")
            );
        }

        for (reason, expected) in [
            ("PDF page has invalid dimensions", "PDF 页面尺寸无效"),
            (
                "PDF page exceeds the rendering budget",
                "PDF 页面超过渲染资源上限",
            ),
            (
                "PDF page exceeds the rendering pixel budget",
                "PDF 页面超过渲染像素上限",
            ),
            (
                "PDF page exceeds the preview memory budget",
                "PDF 页面超过预览内存上限",
            ),
            ("PDF page is too tall", "PDF 页面过高，无法预览"),
            ("PDF page rendering size overflowed", "PDF 页面渲染尺寸溢出"),
        ] {
            assert_eq!(translate(reason).unwrap(), expected);
        }

        assert_eq!(
            translate(
                "Could not render PDF page: Could not inspect rendered PDF page: invalid PNG header"
            )
            .unwrap(),
            "无法渲染 PDF 页面：无法检查已渲染的 PDF 页面：invalid PNG header"
        );
        assert_eq!(
            translate(
                "Could not render PDF page: Could not read rendered PDF page: Permission denied"
            )
            .unwrap(),
            "无法渲染 PDF 页面：无法读取已渲染的 PDF 页面：Permission denied"
        );
        assert_eq!(
            translate("PDF page 17 has invalid layout dimensions").unwrap(),
            "PDF 第 17 页布局尺寸无效"
        );
    }

    #[test]
    fn translates_every_application_owned_office_validation_reason() {
        for (reason, expected) in [
            (
                "LibreOffice did not produce a PDF",
                "LibreOffice 未生成 PDF",
            ),
            (
                "LibreOffice produced more than one output",
                "LibreOffice 生成了多个输出文件",
            ),
            (
                "LibreOffice output is not a regular PDF file",
                "LibreOffice 输出不是非符号链接的普通 PDF 文件",
            ),
            (
                "LibreOffice output is not a PDF file",
                "LibreOffice 输出不是 PDF 文件",
            ),
            (
                "LibreOffice produced an empty PDF",
                "LibreOffice 生成了空 PDF",
            ),
            (
                "LibreOffice output directory identity changed",
                "LibreOffice 输出目录身份已变化",
            ),
            (
                "LibreOffice output escaped the preview workspace",
                "LibreOffice 输出逃离了预览工作区",
            ),
        ] {
            assert_eq!(
                translate(&format!("Could not convert Office document: {reason}")).unwrap(),
                format!("无法转换 Office 文档：{expected}")
            );
        }
        assert_eq!(
            translate(
                "Could not convert Office document: Converted Office PDF is too large to preview (2.0 KiB). Maximum preview size is 128 B."
            )
            .unwrap(),
            "无法转换 Office 文档：转换后的 Office PDF 过大（2.0 KiB），最大预览大小为 128 B"
        );
        for (source, expected) in [
            (
                "Could not convert Office document: Could not inspect LibreOffice output: Permission denied",
                "无法转换 Office 文档：无法检查 LibreOffice 输出：Permission denied",
            ),
            (
                "Could not convert Office document: Could not inspect LibreOffice output directory: No such file or directory",
                "无法转换 Office 文档：无法检查 LibreOffice 输出目录：No such file or directory",
            ),
        ] {
            let translated = translate(source).unwrap();
            assert_eq!(translated, expected);
            assert!(!translated.contains("Could not inspect LibreOffice output"));
        }
    }
}
