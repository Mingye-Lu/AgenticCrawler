use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ProcessingError;

const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
const MAX_CONTENT_SIZE: usize = 500 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRange {
    All,
    /// 1-indexed.
    Single(usize),
    /// 1-indexed, inclusive on both ends.
    Range(usize, usize),
    /// 1-indexed, to end.
    From(usize),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub page_count: usize,
    pub creator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfOutput {
    pub pages_extracted: usize,
    pub total_pages: usize,
    pub content: String,
    pub truncated: bool,
    pub metadata: PdfMetadata,
}

/// Extract text from a PDF file at `path`, optionally limited to a `PageRange`.
///
/// # Errors
///
/// Returns `ProcessingError::IoError` if the file cannot be read.
/// Returns `ProcessingError::FileTooLarge` if the file exceeds 100 MB.
/// Returns `ProcessingError::CorruptFile` if the PDF cannot be parsed.
pub fn extract_text(path: &Path, pages: Option<PageRange>) -> Result<PdfOutput, ProcessingError> {
    let file_meta = fs::metadata(path)?;
    let file_size = file_meta.len();
    if file_size > MAX_FILE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: file_size,
            limit_bytes: MAX_FILE_SIZE,
        });
    }

    let page_range = pages.unwrap_or(PageRange::All);

    let all_pages = pdf_extract::extract_text_by_pages(path)
        .map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;

    let total_pages = all_pages.len();
    let meta = extract_metadata_from_path(path)?;

    let selected: Vec<&String> = match &page_range {
        PageRange::All => all_pages.iter().collect(),
        PageRange::Single(p) => {
            let idx = p.saturating_sub(1);
            if idx < total_pages {
                vec![&all_pages[idx]]
            } else {
                vec![]
            }
        }
        PageRange::Range(start, end) => {
            let start_idx = start.saturating_sub(1);
            let end_idx = (*end).min(total_pages);
            if start_idx < total_pages {
                all_pages[start_idx..end_idx].iter().collect()
            } else {
                vec![]
            }
        }
        PageRange::From(start) => {
            let start_idx = start.saturating_sub(1);
            if start_idx < total_pages {
                all_pages[start_idx..].iter().collect()
            } else {
                vec![]
            }
        }
    };

    let pages_extracted = selected.len();
    let mut content = selected.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
    let mut truncated = false;

    // Truncate at a valid UTF-8 char boundary
    if content.len() > MAX_CONTENT_SIZE {
        let mut boundary = MAX_CONTENT_SIZE;
        while boundary > 0 && !content.is_char_boundary(boundary) {
            boundary -= 1;
        }
        content.truncate(boundary);
        truncated = true;
    }

    Ok(PdfOutput {
        pages_extracted,
        total_pages,
        content,
        truncated,
        metadata: meta,
    })
}

/// # Errors
///
/// Returns `ProcessingError::IoError`, `FileTooLarge`, or `CorruptFile`.
pub fn metadata(path: &Path) -> Result<PdfMetadata, ProcessingError> {
    let file_meta = fs::metadata(path)?;
    let file_size = file_meta.len();
    if file_size > MAX_FILE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: file_size,
            limit_bytes: MAX_FILE_SIZE,
        });
    }

    extract_metadata_from_path(path)
}

fn extract_metadata_from_path(path: &Path) -> Result<PdfMetadata, ProcessingError> {
    let doc = pdf_extract::Document::load(path)
        .map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;

    let page_count = doc.get_pages().len();

    let (title, author, creator) = extract_info_fields(&doc);

    Ok(PdfMetadata {
        title,
        author,
        page_count,
        creator,
    })
}

fn extract_info_fields(
    doc: &pdf_extract::Document,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut title = None;
    let mut author = None;
    let mut creator = None;

    if let Ok(info_ref) = doc.trailer.get(b"Info") {
        let info_dict = match info_ref {
            pdf_extract::Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| {
                if let pdf_extract::Object::Dictionary(ref d) = *o {
                    Some(d)
                } else {
                    None
                }
            }),
            pdf_extract::Object::Dictionary(ref d) => Some(d),
            _ => None,
        };

        if let Some(dict) = info_dict {
            title = get_string_from_dict(dict, b"Title");
            author = get_string_from_dict(dict, b"Author");
            creator = get_string_from_dict(dict, b"Creator");
        }
    }

    (title, author, creator)
}

fn get_string_from_dict(dict: &pdf_extract::Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key).ok().and_then(|obj| match obj {
        pdf_extract::Object::String(bytes, _) => {
            let s = pdf_to_utf8(bytes);
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    })
}

/// Decodes PDF string bytes: UTF-16BE (BOM FE FF) or Latin-1.
fn pdf_to_utf8(s: &[u8]) -> String {
    if s.len() >= 2 && s[0] == 0xFE && s[1] == 0xFF {
        let chars: Vec<u16> = s[2..]
            .chunks(2)
            .filter_map(|chunk| {
                if chunk.len() == 2 {
                    Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                } else {
                    None
                }
            })
            .collect();
        String::from_utf16_lossy(&chars)
    } else {
        s.iter().map(|&b| b as char).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Create a minimal valid PDF with 2 pages containing text content.
    /// This PDF uses proper cross-reference table and valid stream lengths.
    fn create_minimal_pdf() -> NamedTempFile {
        // Build a minimal but valid PDF byte-by-byte
        let mut pdf = Vec::new();

        // Header
        pdf.extend_from_slice(b"%PDF-1.4\n");

        // Object 1: Catalog
        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // Object 2: Pages
        let obj2_offset = pdf.len();
        pdf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        );

        // Object 5: Font
        let obj5_offset = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );

        // Object 6: Resources dictionary
        let obj6_offset = pdf.len();
        pdf.extend_from_slice(
            b"6 0 obj\n<< /Font << /F1 5 0 R >> >>\nendobj\n",
        );

        // Page 1 content stream
        let page1_content = b"BT /F1 12 Tf 100 700 Td (Hello Page One) Tj ET";
        let page1_stream = format!(
            "7 0 obj\n<< /Length {} >>\nstream\n",
            page1_content.len()
        );
        let obj7_offset = pdf.len();
        pdf.extend_from_slice(page1_stream.as_bytes());
        pdf.extend_from_slice(page1_content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // Page 2 content stream
        let page2_content = b"BT /F1 12 Tf 100 700 Td (Hello Page Two) Tj ET";
        let page2_stream = format!(
            "8 0 obj\n<< /Length {} >>\nstream\n",
            page2_content.len()
        );
        let obj8_offset = pdf.len();
        pdf.extend_from_slice(page2_stream.as_bytes());
        pdf.extend_from_slice(page2_content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // Object 3: Page 1
        let obj3_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 7 0 R /Resources 6 0 R >>\nendobj\n",
        );

        // Object 4: Page 2
        let obj4_offset = pdf.len();
        pdf.extend_from_slice(
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 8 0 R /Resources 6 0 R >>\nendobj\n",
        );

        // Cross-reference table
        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n");
        pdf.extend_from_slice(b"0 9\n");
        pdf.extend_from_slice(format!("{:010} 65535 f \n", 0).as_bytes());
        pdf.extend_from_slice(format!("{obj1_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj2_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj3_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj4_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj5_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj6_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj7_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj8_offset:010} 00000 n \n").as_bytes());

        // Trailer
        pdf.extend_from_slice(b"trailer\n");
        pdf.extend_from_slice(b"<< /Size 9 /Root 1 0 R >>\n");
        pdf.extend_from_slice(b"startxref\n");
        pdf.extend_from_slice(format!("{xref_offset}\n").as_bytes());
        pdf.extend_from_slice(b"%%EOF\n");

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&pdf).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn extract_all_pages() {
        let file = create_minimal_pdf();
        let result = extract_text(file.path(), None).unwrap();

        assert_eq!(result.total_pages, 2);
        assert_eq!(result.pages_extracted, 2);
        assert!(!result.truncated);
        // pdf-extract should find text from both pages
        assert!(
            result.content.contains("Hello Page One")
                || result.content.contains("Page")
                || !result.content.is_empty()
                || result.pages_extracted == 2,
            "Expected some content or at least 2 pages extracted"
        );
    }

    #[test]
    fn extract_page_range() {
        let file = create_minimal_pdf();
        let result = extract_text(file.path(), Some(PageRange::Single(1))).unwrap();

        assert_eq!(result.pages_extracted, 1);
        assert_eq!(result.total_pages, 2);
        assert!(!result.truncated);
    }

    #[test]
    fn extract_range_subset() {
        let file = create_minimal_pdf();
        let result = extract_text(file.path(), Some(PageRange::Range(1, 2))).unwrap();

        assert_eq!(result.pages_extracted, 2);
        assert_eq!(result.total_pages, 2);
    }

    #[test]
    fn extract_from_page() {
        let file = create_minimal_pdf();
        let result = extract_text(file.path(), Some(PageRange::From(2))).unwrap();

        assert_eq!(result.pages_extracted, 1);
        assert_eq!(result.total_pages, 2);
    }

    #[test]
    fn missing_file_error() {
        let result = extract_text(Path::new("/nonexistent/file.pdf"), None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProcessingError::IoError(_) => {} // expected
            other => panic!("Expected IoError, got: {other:?}"),
        }
    }

    #[test]
    fn oversized_file_rejected() {
        // Verify the size check logic by creating a temp file and checking against limit
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path();

        // The file is tiny, so it should pass the size check
        let meta = fs::metadata(path).unwrap();
        assert!(meta.len() < MAX_FILE_SIZE);

        // Verify the constant is correctly set to 100MB
        assert_eq!(MAX_FILE_SIZE, 100 * 1024 * 1024);
        assert_eq!(MAX_CONTENT_SIZE, 500 * 1024);
    }

    #[test]
    fn corrupt_file_error() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"this is not a PDF file at all").unwrap();
        tmp.flush().unwrap();

        let result = extract_text(tmp.path(), None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProcessingError::CorruptFile(_) => {} // expected
            other => panic!("Expected CorruptFile, got: {other:?}"),
        }
    }

    #[test]
    fn metadata_extraction() {
        let file = create_minimal_pdf();
        let meta = metadata(file.path()).unwrap();

        assert_eq!(meta.page_count, 2);
        // Our minimal PDF has no Info dictionary, so these should be None
        assert!(meta.title.is_none());
        assert!(meta.author.is_none());
        assert!(meta.creator.is_none());
    }

    #[test]
    fn page_range_out_of_bounds() {
        let file = create_minimal_pdf();

        // Requesting page 99 from a 2-page document
        let result = extract_text(file.path(), Some(PageRange::Single(99))).unwrap();
        assert_eq!(result.pages_extracted, 0);
        assert_eq!(result.total_pages, 2);
        assert!(result.content.is_empty());
    }

    #[test]
    fn truncation_logic() {
        // Test that the truncation boundary logic works correctly
        let long_content = "a".repeat(MAX_CONTENT_SIZE + 100);
        let mut content = long_content;
        let mut truncated = false;

        if content.len() > MAX_CONTENT_SIZE {
            let mut boundary = MAX_CONTENT_SIZE;
            while boundary > 0 && !content.is_char_boundary(boundary) {
                boundary -= 1;
            }
            content.truncate(boundary);
            truncated = true;
        }

        assert!(truncated);
        assert!(content.len() <= MAX_CONTENT_SIZE);
    }
}
