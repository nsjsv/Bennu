use std::fs;
use std::io::Write;
use std::path::Path;

use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::{extract_zipped_xml_text, ZippedXmlDocumentKind};
use crate::extractor::ExtractionStatus;

const WORD_DOCUMENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:r><w:t>alpha heading</w:t></w:r></w:p>
<w:p><w:r><w:t>beta body</w:t><w:t> with tail</w:t></w:r></w:p>
</w:body></w:document>"#;

const SHARED_STRINGS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="4">
<si><t>first shared</t></si>
<si><r><t>rich</t></r><r><t> run</t></r></si>
</sst>"#;

const SHEET_ONE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>inline cell</t></is></c><c r="C1"><v>42</v></c><c r="D1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2"><v>3.14</v></c><c r="B2" t="s"><v>9</v></c></row>
</sheetData></worksheet>"#;

fn slide_xml(title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld>
<p:sp><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp>
</p:cSld></p:sld>"#
    )
}

const NOTES_SLIDE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
         xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:txBody><a:p><a:t>speaker note</a:t></a:p></p:txBody></p:notes>"#;

const ODT_CONTENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:text>
<text:h>Report title</text:h>
<text:p>first para<text:span> with span</text:span></text:p>
<text:p/>
</office:text></office:body></office:document-content>"#;

fn write_zip(document_path: &Path, entries: &[(&str, String)]) {
    let file = fs::File::create(document_path).unwrap();
    let mut writer = ZipWriter::new(file);
    for (entry_name, contents) in entries {
        writer
            .start_file(*entry_name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

fn extract(
    document_path: &Path,
    document_kind: ZippedXmlDocumentKind,
    max_output_bytes: u64,
) -> ExtractionStatus {
    extract_zipped_xml_text(document_path, document_kind, max_output_bytes)
        .expect("fixture extraction must not fail the crawl pipeline")
        .status
}

fn extracted_text(document_path: &Path, document_kind: ZippedXmlDocumentKind) -> String {
    let outcome = extract_zipped_xml_text(document_path, document_kind, 4096).unwrap();
    assert_eq!(outcome.status, ExtractionStatus::Indexed);
    outcome.text.expect("indexed text must be present")
}

#[test]
fn word_documents_capture_paragraph_text_with_breaks() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("report.docx");
    write_zip(
        &document_path,
        &[("word/document.xml", WORD_DOCUMENT_XML.to_owned())],
    );

    assert_eq!(
        extracted_text(&document_path, ZippedXmlDocumentKind::WordDocument),
        "alpha heading\nbeta body with tail\n"
    );
}

#[test]
fn spreadsheets_resolve_shared_strings_and_index_numbers() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("workbook.xlsx");
    write_zip(
        &document_path,
        &[
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML.to_owned()),
            ("xl/worksheets/sheet1.xml", SHEET_ONE_XML.to_owned()),
        ],
    );

    assert_eq!(
        extracted_text(&document_path, ZippedXmlDocumentKind::Spreadsheet),
        "first shared\tinline cell\t42\trich run\t\n3.14\t\n"
    );
}

#[test]
fn presentations_order_slides_numerically_and_include_notes() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("slides.pptx");
    write_zip(
        &document_path,
        &[
            ("ppt/slides/slide10.xml", slide_xml("tenth slide")),
            ("ppt/slides/slide2.xml", slide_xml("second slide")),
            ("ppt/slides/slide1.xml", slide_xml("first slide")),
            (
                "ppt/notesSlides/notesSlide1.xml",
                NOTES_SLIDE_XML.to_owned(),
            ),
        ],
    );

    assert_eq!(
        extracted_text(&document_path, ZippedXmlDocumentKind::Presentation),
        "first slide\nsecond slide\ntenth slide\nspeaker note\n"
    );
}

#[test]
fn open_documents_capture_block_text() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("notes.odt");
    write_zip(
        &document_path,
        &[("content.xml", ODT_CONTENT_XML.to_owned())],
    );

    assert_eq!(
        extracted_text(&document_path, ZippedXmlDocumentKind::OpenDocumentText),
        "Report title\nfirst para with span\n"
    );
}

#[test]
fn non_office_payloads_degrade_to_read_failed() {
    let directory = tempdir().unwrap();

    let corrupt_path = directory.path().join("corrupt.docx");
    fs::write(&corrupt_path, b"definitely not a zip container").unwrap();
    assert!(matches!(
        extract_status(&corrupt_path, ZippedXmlDocumentKind::WordDocument),
        ExtractionStatus::ReadFailed { .. }
    ));

    let missing_entry_path = directory.path().join("empty.docx");
    write_zip(&missing_entry_path, &[("docProps/core.xml", String::new())]);
    assert!(matches!(
        extract_status(&missing_entry_path, ZippedXmlDocumentKind::WordDocument),
        ExtractionStatus::ReadFailed { .. }
    ));

    let malformed_path = directory.path().join("malformed.docx");
    write_zip(
        &malformed_path,
        &[(
            "word/document.xml",
            "<w:document><w:p></w:document>".to_owned(),
        )],
    );
    assert!(matches!(
        extract_status(&malformed_path, ZippedXmlDocumentKind::WordDocument),
        ExtractionStatus::ReadFailed { .. }
    ));
}

fn extract_status(document_path: &Path, document_kind: ZippedXmlDocumentKind) -> ExtractionStatus {
    extract(document_path, document_kind, 4096)
}

#[test]
fn oversized_documents_report_too_large() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("long.docx");
    write_zip(
        &document_path,
        &[("word/document.xml", WORD_DOCUMENT_XML.to_owned())],
    );

    let status = extract(&document_path, ZippedXmlDocumentKind::WordDocument, 8);
    assert_eq!(status, ExtractionStatus::TooLarge);
}

#[test]
fn oversize_shared_string_table_is_bounded() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("big-shared.xlsx");
    let oversized_shared_strings = format!(
        r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{}</sst>"#,
        "<si><t>filler text</t></si>".repeat(400)
    );
    write_zip(
        &document_path,
        &[
            ("xl/sharedStrings.xml", oversized_shared_strings),
            ("xl/worksheets/sheet1.xml", SHEET_ONE_XML.to_owned()),
        ],
    );

    let status = extract(&document_path, ZippedXmlDocumentKind::Spreadsheet, 1024);
    assert_eq!(status, ExtractionStatus::TooLarge);
}

#[test]
fn missing_zip_file_is_skipped_not_inaccessible() {
    let directory = tempdir().unwrap();
    let document_path = directory.path().join("gone.docx");
    fs::write(&document_path, b"payload").unwrap();
    fs::remove_file(&document_path).unwrap();

    let outcome =
        extract_zipped_xml_text(&document_path, ZippedXmlDocumentKind::WordDocument, 4096).unwrap();

    assert!(matches!(
        outcome.status,
        ExtractionStatus::ReadFailed { .. }
    ));
}
