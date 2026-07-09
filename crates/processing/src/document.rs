use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::error::ProcessingError;

const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
const MAX_CONTENT_SIZE: usize = 500 * 1024;
const MAX_XML_UNCOMPRESSED: u64 = 50 * 1024 * 1024;

/// Metadata extracted from a document file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// Result of document text extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentOutput {
    pub format: String,
    pub content: String,
    pub word_count: usize,
    pub truncated: bool,
    pub metadata: DocumentMetadata,
}

/// Extract plain text from a document file.
///
/// Supported formats: `.docx`, `.pptx`, `.epub`, `.rtf`, `.odt`.
pub fn extract_text(path: &Path) -> Result<DocumentOutput, ProcessingError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let format = match ext.as_str() {
        "docx" | "pptx" | "epub" | "odt" | "rtf" => ext.clone(),
        other => {
            return Err(ProcessingError::UnsupportedFormat(format!(
                ".{other} is not a supported document format"
            )));
        }
    };

    let file_meta = std::fs::metadata(path)?;
    if file_meta.len() > MAX_FILE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: file_meta.len(),
            limit_bytes: MAX_FILE_SIZE,
        });
    }

    let (raw_content, metadata) = match format.as_str() {
        "rtf" => (extract_rtf_text(path)?, DocumentMetadata::default()),
        _ => extract_from_zip(path, &format)?,
    };

    let mut content = raw_content;
    let truncated = content.len() > MAX_CONTENT_SIZE;
    if truncated {
        let mut boundary = MAX_CONTENT_SIZE;
        while boundary > 0 && !content.is_char_boundary(boundary) {
            boundary -= 1;
        }
        content.truncate(boundary);
        if let Some(pos) = content.rfind(' ') {
            content.truncate(pos);
        }
    }

    let word_count = content.split_whitespace().count();

    Ok(DocumentOutput {
        format,
        content,
        word_count,
        truncated,
        metadata,
    })
}

fn extract_from_zip(
    path: &Path,
    format: &str,
) -> Result<(String, DocumentMetadata), ProcessingError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| ProcessingError::CorruptFile(format!("Invalid ZIP archive: {e}")))?;

    let metadata = extract_zip_metadata(&mut archive, format);

    let text = match format {
        "docx" => extract_docx_text(&mut archive)?,
        "pptx" => extract_pptx_text(&mut archive)?,
        "epub" => extract_epub_text(&mut archive)?,
        "odt" => extract_odt_text(&mut archive)?,
        _ => {
            return Err(ProcessingError::UnsupportedFormat(format!(
                ".{format} is not supported"
            )));
        }
    };

    Ok((text, metadata))
}

fn extract_zip_metadata<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    format: &str,
) -> DocumentMetadata {
    let meta_path = match format {
        "docx" | "pptx" => "docProps/core.xml",
        "odt" => "meta.xml",
        _ => return DocumentMetadata::default(),
    };

    let Ok(entry) = archive.by_name(meta_path) else {
        return DocumentMetadata::default();
    };

    let mut xml = String::new();
    if entry
        .take(MAX_XML_UNCOMPRESSED)
        .read_to_string(&mut xml)
        .is_err()
    {
        return DocumentMetadata::default();
    }

    parse_core_metadata(&xml)
}

fn parse_core_metadata(xml: &str) -> DocumentMetadata {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut meta = DocumentMetadata::default();
    let mut current_tag = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                current_tag = name;
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if text.trim().is_empty() {
                    buf.clear();
                    continue;
                }
                match current_tag.as_str() {
                    "title" => meta.title = Some(text),
                    "creator" | "author" => meta.author = Some(text),
                    "created" => meta.created = Some(text),
                    "modified" => meta.modified = Some(text),
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    meta
}

fn extract_docx_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<String, ProcessingError> {
    let entry = archive
        .by_name("word/document.xml")
        .map_err(|_| ProcessingError::CorruptFile("Missing word/document.xml".into()))?;

    if entry.size() > MAX_XML_UNCOMPRESSED {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: entry.size(),
            limit_bytes: MAX_XML_UNCOMPRESSED,
        });
    }

    Ok(strip_xml_to_text(entry.take(MAX_XML_UNCOMPRESSED)))
}

fn extract_pptx_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<String, ProcessingError> {
    let slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            let name = entry.name().to_string();
            if name.starts_with("ppt/slides/slide")
                && Path::new(&name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
            {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut all_text = String::new();
    for name in &slide_names {
        if let Ok(entry) = archive.by_name(name) {
            if entry.size() > MAX_XML_UNCOMPRESSED {
                continue;
            }
            let slide_text = strip_xml_to_text(entry.take(MAX_XML_UNCOMPRESSED));
            if !slide_text.is_empty() {
                if !all_text.is_empty() {
                    all_text.push(' ');
                }
                all_text.push_str(&slide_text);
            }
        }
    }

    if all_text.is_empty() {
        return Err(ProcessingError::CorruptFile(
            "No slide content found in PPTX".into(),
        ));
    }
    Ok(all_text)
}

fn extract_epub_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<String, ProcessingError> {
    let html_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            let name = entry.name().to_string();
            let ext_matches = Path::new(&name).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("xhtml")
                    || ext.eq_ignore_ascii_case("html")
                    || ext.eq_ignore_ascii_case("htm")
            });
            if ext_matches {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut all_text = String::new();
    for name in &html_names {
        if let Ok(entry) = archive.by_name(name) {
            if entry.size() > MAX_XML_UNCOMPRESSED {
                continue;
            }
            let page_text = strip_xml_to_text(entry.take(MAX_XML_UNCOMPRESSED));
            if !page_text.is_empty() {
                if !all_text.is_empty() {
                    all_text.push(' ');
                }
                all_text.push_str(&page_text);
                if all_text.len() >= MAX_CONTENT_SIZE {
                    all_text.truncate(MAX_CONTENT_SIZE);
                    break;
                }
            }
        }
    }

    if all_text.is_empty() {
        return Err(ProcessingError::CorruptFile(
            "No HTML content found in EPUB".into(),
        ));
    }
    Ok(all_text)
}

fn extract_odt_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<String, ProcessingError> {
    let entry = archive
        .by_name("content.xml")
        .map_err(|_| ProcessingError::CorruptFile("Missing content.xml".into()))?;

    if entry.size() > MAX_XML_UNCOMPRESSED {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: entry.size(),
            limit_bytes: MAX_XML_UNCOMPRESSED,
        });
    }

    Ok(strip_xml_to_text(entry.take(MAX_XML_UNCOMPRESSED)))
}

fn extract_rtf_text(path: &Path) -> Result<String, ProcessingError> {
    let raw = std::fs::read_to_string(path)?;
    if !raw.starts_with("{\\rtf") {
        return Err(ProcessingError::CorruptFile("Not a valid RTF file".into()));
    }

    let mut result = String::new();
    let mut in_control = false;
    let mut brace_depth: i32 = 0;
    let mut skip_group = false;
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '{' => {
                brace_depth += 1;
                let rest: String = chars[i..].iter().take(20).collect();
                if rest.contains("\\fonttbl")
                    || rest.contains("\\colortbl")
                    || rest.contains("\\stylesheet")
                    || rest.contains("\\info")
                {
                    skip_group = true;
                }
            }
            '}' => {
                brace_depth -= 1;
                if brace_depth <= 0 {
                    skip_group = false;
                }
                if skip_group && brace_depth <= 1 {
                    skip_group = false;
                }
            }
            '\\' if !skip_group => {
                in_control = true;
                if i + 1 < chars.len() {
                    let next = chars[i + 1];
                    match next {
                        '\'' => {
                            if i + 3 < chars.len() {
                                let hex: String = chars[i + 2..i + 4].iter().collect();
                                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                                    result.push(char::from(byte));
                                }
                            }
                            i += 3;
                            in_control = false;
                        }
                        '{' | '}' | '\\' => {
                            result.push(next);
                            i += 1;
                            in_control = false;
                        }
                        '\n' | '\r' => {
                            result.push(' ');
                            i += 1;
                            in_control = false;
                        }
                        _ => {
                            i += 1;
                            while i < chars.len()
                                && chars[i] != ' '
                                && chars[i] != '\\'
                                && chars[i] != '{'
                                && chars[i] != '}'
                                && chars[i] != '\n'
                                && chars[i] != '\r'
                            {
                                i += 1;
                            }
                            if i < chars.len() && chars[i] == ' ' {
                                i += 1;
                            }
                            in_control = false;
                            continue;
                        }
                    }
                }
            }
            _ if skip_group || in_control => {}
            '\n' | '\r' => {
                if !result.ends_with(' ') {
                    result.push(' ');
                }
            }
            _ => {
                result.push(ch);
            }
        }
        i += 1;
    }

    let cleaned: String = result.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(cleaned)
}

fn strip_xml_to_text(reader: impl Read) -> String {
    const MAX_OUTPUT_SIZE: usize = MAX_CONTENT_SIZE * 2;

    let mut reader = Reader::from_reader(std::io::BufReader::new(reader));
    reader.config_mut().trim_text(true);

    let mut text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                let t = e.unescape().unwrap_or_default();
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
                if text.len() >= MAX_OUTPUT_SIZE {
                    break;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_minimal_docx(text: &str) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".docx").unwrap();
        {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let mut zip = zip::ZipWriter::new(&mut f);
            zip.start_file("word/document.xml", opts).unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
            )
            .unwrap();
            zip.start_file("docProps/core.xml", opts).unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/"><dc:title>Test Doc</dc:title><dc:creator>Test Author</dc:creator><dcterms:created>2025-01-01</dcterms:created></cp:coreProperties>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }
        f
    }

    #[test]
    fn extract_docx_basic() {
        let f = create_minimal_docx("Hello DOCX World");
        let result = extract_text(f.path()).unwrap();

        assert_eq!(result.format, "docx");
        assert!(result.content.contains("Hello DOCX World"));
        assert_eq!(result.word_count, 3);
        assert!(!result.truncated);
        assert_eq!(result.metadata.title.as_deref(), Some("Test Doc"));
        assert_eq!(result.metadata.author.as_deref(), Some("Test Author"));
    }

    #[test]
    fn extract_docx_multiword() {
        let f = create_minimal_docx("The quick brown fox jumps over the lazy dog");
        let result = extract_text(f.path()).unwrap();

        assert_eq!(result.word_count, 9);
        assert!(result.content.contains("quick brown fox"));
    }

    #[test]
    fn unsupported_format_returns_error() {
        let f = NamedTempFile::with_suffix(".doc").unwrap();
        let result = extract_text(f.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ProcessingError::UnsupportedFormat(_)),
            "Expected UnsupportedFormat, got: {err:?}"
        );
    }

    #[test]
    fn file_too_large_returns_error() {
        let err = ProcessingError::FileTooLarge {
            actual_bytes: 200_000_000,
            limit_bytes: MAX_FILE_SIZE,
        };
        assert!(err.to_string().contains("200000000"));
    }

    #[test]
    fn extract_odt_basic() {
        let mut f = NamedTempFile::with_suffix(".odt").unwrap();
        {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let mut zip = zip::ZipWriter::new(&mut f);
            zip.start_file("content.xml", opts).unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text><text:p>Hello ODT World</text:p></office:text></office:body></office:document-content>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let result = extract_text(f.path()).unwrap();
        assert_eq!(result.format, "odt");
        assert!(result.content.contains("Hello ODT World"));
    }

    #[test]
    fn extract_pptx_basic() {
        let mut f = NamedTempFile::with_suffix(".pptx").unwrap();
        {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let mut zip = zip::ZipWriter::new(&mut f);
            zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Slide One Content</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let result = extract_text(f.path()).unwrap();
        assert_eq!(result.format, "pptx");
        assert!(result.content.contains("Slide One Content"));
    }

    #[test]
    fn extract_epub_basic() {
        let mut f = NamedTempFile::with_suffix(".epub").unwrap();
        {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let mut zip = zip::ZipWriter::new(&mut f);
            zip.start_file("OEBPS/chapter1.xhtml", opts).unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ch1</title></head><body><p>EPUB chapter text here</p></body></html>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let result = extract_text(f.path()).unwrap();
        assert_eq!(result.format, "epub");
        assert!(result.content.contains("EPUB chapter text here"));
    }

    #[test]
    fn extract_rtf_basic() {
        let mut f = NamedTempFile::with_suffix(".rtf").unwrap();
        write!(f, r"{{\rtf1\ansi\deff0 Hello RTF World}}").unwrap();
        f.flush().unwrap();

        let result = extract_text(f.path()).unwrap();
        assert_eq!(result.format, "rtf");
        assert!(
            result.content.contains("Hello RTF World"),
            "Content was: {:?}",
            result.content
        );
    }

    #[test]
    fn corrupt_zip_returns_error() {
        let mut f = NamedTempFile::with_suffix(".docx").unwrap();
        write!(f, "this is not a zip file").unwrap();
        f.flush().unwrap();

        let result = extract_text(f.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ProcessingError::CorruptFile(_)),
            "Expected CorruptFile, got: {err:?}"
        );
    }

    #[test]
    fn strip_xml_to_text_basic() {
        let xml = r"<root><p>Hello</p><p>World</p></root>";
        let text = strip_xml_to_text(xml.as_bytes());
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn strip_xml_handles_nested_tags() {
        let xml = r"<w:p><w:r><w:t>foo</w:t></w:r><w:r><w:t>bar</w:t></w:r></w:p>";
        let text = strip_xml_to_text(xml.as_bytes());
        assert_eq!(text, "foo bar");
    }

    #[test]
    fn strip_xml_to_text_caps_output() {
        let mut xml = String::from("<root>");
        let node = "<p>word</p>";
        let needed = (MAX_CONTENT_SIZE * 2) / node.len() + 100;
        for _ in 0..needed {
            xml.push_str(node);
        }
        xml.push_str("</root>");

        let text = strip_xml_to_text(xml.as_bytes());
        assert!(
            text.len() <= MAX_CONTENT_SIZE * 2 + "word".len(),
            "expected capped output, got length {}",
            text.len()
        );
    }
}
