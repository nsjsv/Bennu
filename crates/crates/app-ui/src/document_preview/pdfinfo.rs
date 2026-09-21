use super::model::{DocumentPageSize, MAX_DOCUMENT_PAGES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PdfInfoSummary {
    pub(crate) page_count: usize,
    pub(crate) encrypted: bool,
}

pub(crate) fn parse_pdfinfo_summary(output: &[u8]) -> Result<PdfInfoSummary, String> {
    let output = std::str::from_utf8(output)
        .map_err(|_| "Poppler returned non-UTF-8 PDF metadata".to_owned())?;
    let mut page_count = None;
    let mut encrypted = None;

    for line in output.lines() {
        if let Some(value) = line.strip_prefix("Pages:") {
            if page_count.is_some() {
                return Err("Poppler returned duplicate PDF page counts".to_owned());
            }
            let value = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Poppler returned an invalid PDF page count".to_owned())?;
            if value == 0 {
                return Err("PDF preview requires at least one page".to_owned());
            }
            if value > MAX_DOCUMENT_PAGES {
                return Err(format!(
                    "PDF preview supports at most {MAX_DOCUMENT_PAGES} pages"
                ));
            }
            page_count = Some(value);
        } else if let Some(value) = line.strip_prefix("Encrypted:") {
            if encrypted.is_some() {
                return Err("Poppler returned duplicate PDF encryption metadata".to_owned());
            }
            let value = value.trim();
            encrypted = Some(if value == "no" {
                false
            } else if value == "yes" || value.starts_with("yes ") {
                true
            } else {
                return Err("Poppler returned invalid PDF encryption metadata".to_owned());
            });
        }
    }

    Ok(PdfInfoSummary {
        page_count: page_count.ok_or_else(|| "Poppler omitted the PDF page count".to_owned())?,
        encrypted: encrypted.ok_or_else(|| "Poppler omitted PDF encryption metadata".to_owned())?,
    })
}

pub(crate) fn parse_pdfinfo_pages(
    output: &[u8],
    expected_page_count: usize,
) -> Result<Vec<DocumentPageSize>, String> {
    if expected_page_count == 0 || expected_page_count > MAX_DOCUMENT_PAGES {
        return Err("Invalid expected PDF page count".to_owned());
    }
    let output = std::str::from_utf8(output)
        .map_err(|_| "Poppler returned non-UTF-8 PDF page metadata".to_owned())?;
    let mut crop_boxes = vec![None; expected_page_count];
    let mut rotations = vec![None; expected_page_count];

    for line in output.lines() {
        let Some(rest) = line.strip_prefix("Page") else {
            continue;
        };
        let fields = rest.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let Ok(page_number) = fields[0].parse::<usize>() else {
            continue;
        };
        if !(1..=expected_page_count).contains(&page_number) {
            return Err(format!(
                "Poppler returned unexpected PDF page {page_number}"
            ));
        }
        let index = page_number - 1;
        match fields[1] {
            "CropBox:" => {
                if fields.len() != 6 || crop_boxes[index].is_some() {
                    return Err(format!(
                        "Poppler returned invalid or duplicate CropBox for PDF page {page_number}"
                    ));
                }
                let coordinates = fields[2..]
                    .iter()
                    .map(|value| {
                        value.parse::<f64>().map_err(|_| {
                            format!("Poppler returned invalid CropBox for PDF page {page_number}")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if coordinates.iter().any(|value| !value.is_finite()) {
                    return Err(format!(
                        "Poppler returned non-finite CropBox for PDF page {page_number}"
                    ));
                }
                let width = coordinates[2] - coordinates[0];
                let height = coordinates[3] - coordinates[1];
                if width <= 0.0 || height <= 0.0 {
                    return Err(format!(
                        "Poppler returned non-positive CropBox for PDF page {page_number}"
                    ));
                }
                crop_boxes[index] = Some((width, height));
            }
            "rot:" => {
                if fields.len() != 3 || rotations[index].is_some() {
                    return Err(format!(
                        "Poppler returned invalid or duplicate rotation for PDF page {page_number}"
                    ));
                }
                let rotation = fields[2].parse::<i32>().map_err(|_| {
                    format!("Poppler returned invalid rotation for PDF page {page_number}")
                })?;
                if !matches!(rotation, 0 | 90 | 180 | 270) {
                    return Err(format!(
                        "Poppler returned unsupported rotation for PDF page {page_number}"
                    ));
                }
                rotations[index] = Some(rotation);
            }
            _ => {}
        }
    }

    crop_boxes
        .into_iter()
        .zip(rotations)
        .enumerate()
        .map(|(index, (crop_box, rotation))| {
            let page_number = index + 1;
            let (width, height) = crop_box
                .ok_or_else(|| format!("Poppler omitted CropBox for PDF page {page_number}"))?;
            let rotation = rotation
                .ok_or_else(|| format!("Poppler omitted rotation for PDF page {page_number}"))?;
            DocumentPageSize::from_crop_box(width, height, rotation)
                .map_err(|_| format!("PDF page has an invalid CropBox on page {page_number}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_summary_and_rejects_missing_or_excessive_counts() {
        assert_eq!(
            parse_pdfinfo_summary(b"Pages:           2\nEncrypted:       no\n").unwrap(),
            PdfInfoSummary {
                page_count: 2,
                encrypted: false,
            }
        );
        assert!(parse_pdfinfo_summary(b"Encrypted: no\n").is_err());
        assert!(parse_pdfinfo_summary(b"Pages: 0\nEncrypted: no\n").is_err());
        assert!(parse_pdfinfo_summary(b"Pages: 10001\nEncrypted: no\n").is_err());
        assert!(parse_pdfinfo_summary(b"Pages: 1\nPages: 1\nEncrypted: no\n").is_err());
    }

    #[test]
    fn recognizes_encrypted_pdf_metadata() {
        assert!(
            parse_pdfinfo_summary(b"Pages: 1\nEncrypted: yes (print:yes)\n")
                .unwrap()
                .encrypted
        );
    }

    #[test]
    fn parses_crop_boxes_and_applies_quarter_turn_rotation() {
        let pages = parse_pdfinfo_pages(
            b"Page    1 rot: 0\nPage    1 CropBox: 10 20 610 820\n\
              Page    2 CropBox: 25 50 425 650\nPage    2 rot: 90\n",
            2,
        )
        .unwrap();

        assert_eq!(
            pages[0],
            DocumentPageSize::from_crop_box(600.0, 800.0, 0).unwrap()
        );
        assert_eq!((pages[1].width, pages[1].height), (600.0, 400.0));
        assert!(pages[1].quarter_turn);
    }

    #[test]
    fn rejects_missing_duplicate_non_finite_and_invalid_page_metadata() {
        assert!(parse_pdfinfo_pages(b"Page 1 rot: 0\n", 1).is_err());
        assert!(parse_pdfinfo_pages(
            b"Page 1 rot: 0\nPage 1 rot: 90\nPage 1 CropBox: 0 0 10 10\n",
            1
        )
        .is_err());
        assert!(parse_pdfinfo_pages(b"Page 1 rot: 0\nPage 1 CropBox: 0 0 NaN 10\n", 1).is_err());
        assert!(parse_pdfinfo_pages(b"Page 1 rot: 45\nPage 1 CropBox: 0 0 10 10\n", 1).is_err());
        assert!(parse_pdfinfo_pages(b"Page 1 rot: 0\nPage 1 CropBox: 10 0 0 10\n", 1).is_err());
        assert!(parse_pdfinfo_pages(b"Page 2 rot: 0\nPage 2 CropBox: 0 0 10 10\n", 1).is_err());
    }
}
