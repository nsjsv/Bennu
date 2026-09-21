use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentPreviewFormat {
    Pdf,
    Office(OfficeDocumentFormat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfficeDocumentFormat {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
    Odt,
    Ods,
    Odp,
}

pub(crate) fn document_preview_format_for_path(path: &Path) -> Option<DocumentPreviewFormat> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("pdf") {
        return Some(DocumentPreviewFormat::Pdf);
    }

    let office = if extension.eq_ignore_ascii_case("doc") {
        OfficeDocumentFormat::Doc
    } else if extension.eq_ignore_ascii_case("docx") {
        OfficeDocumentFormat::Docx
    } else if extension.eq_ignore_ascii_case("xls") {
        OfficeDocumentFormat::Xls
    } else if extension.eq_ignore_ascii_case("xlsx") {
        OfficeDocumentFormat::Xlsx
    } else if extension.eq_ignore_ascii_case("ppt") {
        OfficeDocumentFormat::Ppt
    } else if extension.eq_ignore_ascii_case("pptx") {
        OfficeDocumentFormat::Pptx
    } else if extension.eq_ignore_ascii_case("odt") {
        OfficeDocumentFormat::Odt
    } else if extension.eq_ignore_ascii_case("ods") {
        OfficeDocumentFormat::Ods
    } else if extension.eq_ignore_ascii_case("odp") {
        OfficeDocumentFormat::Odp
    } else {
        return None;
    };
    Some(DocumentPreviewFormat::Office(office))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pdf_and_office_extensions_case_insensitively() {
        assert_eq!(
            document_preview_format_for_path(Path::new("report.PDF")),
            Some(DocumentPreviewFormat::Pdf)
        );
        for (extension, expected) in [
            ("doc", OfficeDocumentFormat::Doc),
            ("docx", OfficeDocumentFormat::Docx),
            ("xls", OfficeDocumentFormat::Xls),
            ("xlsx", OfficeDocumentFormat::Xlsx),
            ("ppt", OfficeDocumentFormat::Ppt),
            ("pptx", OfficeDocumentFormat::Pptx),
            ("odt", OfficeDocumentFormat::Odt),
            ("ods", OfficeDocumentFormat::Ods),
            ("odp", OfficeDocumentFormat::Odp),
        ] {
            for spelling in [extension.to_owned(), extension.to_ascii_uppercase()] {
                assert_eq!(
                    document_preview_format_for_path(Path::new(&format!("preview.{spelling}"))),
                    Some(DocumentPreviewFormat::Office(expected))
                );
            }
        }
    }

    #[test]
    fn rejects_non_document_extensions() {
        assert_eq!(
            document_preview_format_for_path(Path::new("notes.txt")),
            None
        );
        assert_eq!(
            document_preview_format_for_path(Path::new("no-extension")),
            None
        );
    }
}
