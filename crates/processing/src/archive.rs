use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::error::ProcessingError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub size: u64,
    pub compressed_size: Option<u64>,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveOutput {
    pub format: String,
    pub total_files: usize,
    pub total_size_bytes: u64,
    pub entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntry {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub content: Option<String>,
}

const MAX_ARCHIVE_SIZE: u64 = 1024 * 1024 * 1024;

const MAX_ENTRY_SIZE: u64 = 100 * 1024 * 1024;

const MAX_INLINE_CONTENT_SIZE: u64 = 50 * 1024;

/// Validate that an entry path is safe (no path traversal).
fn is_safe_path(entry_name: &str) -> bool {
    let path = std::path::Path::new(entry_name);
    if path.is_absolute() || entry_name.starts_with('/') {
        return false;
    }
    path.components()
        .all(|c| !matches!(c, std::path::Component::ParentDir))
}

/// List all entries in an archive file.
///
/// Supports ZIP on all platforms. TAR formats (`.tar`, `.tar.gz`, `.tgz`,
/// `.tar.bz2`) are only supported on Unix.
///
/// # Errors
///
/// - `FileTooLarge` if archive exceeds 1 GB or total uncompressed content exceeds 1 GB.
/// - `UnsupportedFormat` for unknown archive types.
/// - `CorruptFile` if the archive cannot be parsed.
pub fn list_archive(path: &Path) -> Result<ArchiveOutput, ProcessingError> {
    let file_size = std::fs::metadata(path)?.len();
    if file_size > MAX_ARCHIVE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: file_size,
            limit_bytes: MAX_ARCHIVE_SIZE,
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let format = match ext.as_str() {
        "zip" => "zip",
        "tar" | "tgz" => "tar",
        "gz" => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if std::path::Path::new(stem)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tar"))
            {
                "tar"
            } else {
                return Err(ProcessingError::UnsupportedFormat(format!(
                    "Unsupported archive format: .{ext}"
                )));
            }
        }
        "bz2" => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if std::path::Path::new(stem)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tar"))
            {
                "tar"
            } else {
                return Err(ProcessingError::UnsupportedFormat(format!(
                    "Unsupported archive format: .{ext}"
                )));
            }
        }
        _ => {
            return Err(ProcessingError::UnsupportedFormat(format!(
                "Unsupported archive format: .{ext}"
            )));
        }
    };

    if format == "zip" {
        list_zip(path)
    } else {
        #[cfg(unix)]
        {
            list_tar(path, &ext)
        }
        #[cfg(not(unix))]
        {
            Err(ProcessingError::UnsupportedFormat(
                "TAR archives only supported on Unix".to_string(),
            ))
        }
    }
}

fn list_zip(path: &Path) -> Result<ArchiveOutput, ProcessingError> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;

    let mut entries = Vec::new();
    let mut total_size = 0u64;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;

        total_size = total_size.saturating_add(entry.size());
        if total_size > MAX_ARCHIVE_SIZE {
            return Err(ProcessingError::FileTooLarge {
                actual_bytes: total_size,
                limit_bytes: MAX_ARCHIVE_SIZE,
            });
        }

        entries.push(ArchiveEntry {
            path: entry.name().to_string(),
            size: entry.size(),
            compressed_size: Some(entry.compressed_size()),
            is_directory: entry.is_dir(),
        });
    }

    Ok(ArchiveOutput {
        format: "zip".to_string(),
        total_files: entries.iter().filter(|e| !e.is_directory).count(),
        total_size_bytes: total_size,
        entries,
    })
}

#[cfg(unix)]
fn list_tar(path: &Path, ext: &str) -> Result<ArchiveOutput, ProcessingError> {
    use flate2::read::GzDecoder;
    use std::fs::File;

    let file = File::open(path)?;

    let entries_result: Result<Vec<ArchiveEntry>, ProcessingError> = match ext {
        "tgz" | "gz" => {
            let decoder = GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            collect_tar_entries(&mut archive)
        }
        "tar" => {
            let mut archive = tar::Archive::new(file);
            collect_tar_entries(&mut archive)
        }
        _ => {
            return Err(ProcessingError::UnsupportedFormat(format!(
                "Unsupported tar variant: .{ext}"
            )));
        }
    };

    let entries = entries_result?;
    let total_size: u64 = entries.iter().map(|e| e.size).sum();

    if total_size > MAX_ARCHIVE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: total_size,
            limit_bytes: MAX_ARCHIVE_SIZE,
        });
    }

    Ok(ArchiveOutput {
        format: "tar".to_string(),
        total_files: entries.iter().filter(|e| !e.is_directory).count(),
        total_size_bytes: total_size,
        entries,
    })
}

#[cfg(unix)]
fn collect_tar_entries<R: std::io::Read>(
    archive: &mut tar::Archive<R>,
) -> Result<Vec<ArchiveEntry>, ProcessingError> {
    let mut entries = Vec::new();
    for entry_result in archive
        .entries()
        .map_err(|e| ProcessingError::CorruptFile(e.to_string()))?
    {
        let entry = entry_result.map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| ProcessingError::CorruptFile(e.to_string()))?
            .to_string_lossy()
            .to_string();
        let size = entry.size();
        let is_directory = entry.header().entry_type().is_dir();

        entries.push(ArchiveEntry {
            path,
            size,
            compressed_size: None,
            is_directory,
        });
    }
    Ok(entries)
}

/// Extract a single entry from a ZIP archive to `output_dir`.
///
/// # Security
///
/// - Rejects entry paths containing `..` components (zip-slip prevention).
/// - Rejects entries larger than 100 MB.
/// - Validates that the resolved output path is within `output_dir`.
pub fn extract_entry(
    path: &Path,
    entry_path: &str,
    output_dir: &Path,
) -> Result<ExtractedEntry, ProcessingError> {
    // SECURITY: validate entry_path against path traversal
    if !is_safe_path(entry_path) {
        return Err(ProcessingError::FormatError(format!(
            "Unsafe entry path rejected: {entry_path}"
        )));
    }

    let file = std::fs::File::open(path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| ProcessingError::CorruptFile(e.to_string()))?;

    let mut entry = archive.by_name(entry_path).map_err(|_| {
        ProcessingError::FormatError(format!("Entry not found: {entry_path}"))
    })?;

    let entry_size = entry.size();
    if entry_size > MAX_ENTRY_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: entry_size,
            limit_bytes: MAX_ENTRY_SIZE,
        });
    }

    let dest_path = output_dir.join(entry_path);

    // Ensure dest_path is under output_dir (canonical path check)
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)?;
        let canonical_output = output_dir.canonicalize()?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(&canonical_output) {
            return Err(ProcessingError::FormatError(
                "Path traversal detected".to_string(),
            ));
        }
    }

    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;

    std::fs::write(&dest_path, &buf)?;

    let content = if entry_size < MAX_INLINE_CONTENT_SIZE {
        String::from_utf8(buf).ok()
    } else {
        None
    };

    Ok(ExtractedEntry {
        path: dest_path,
        size: entry_size,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_test_zip() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        {
            let mut zip = zip::ZipWriter::new(&mut f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("hello.txt", opts).unwrap();
            zip.write_all(b"Hello from zip!").unwrap();
            zip.start_file("data/world.txt", opts).unwrap();
            zip.write_all(b"World data").unwrap();
            zip.add_directory("empty_dir/", opts).unwrap();
            zip.finish().unwrap();
        }
        f
    }

    #[test]
    fn test_list_zip() {
        let zip_file = create_test_zip();
        let result = list_archive(zip_file.path()).unwrap();

        assert_eq!(result.format, "zip");
        assert_eq!(result.total_files, 2);
        assert_eq!(result.entries.len(), 3);

        let hello = result.entries.iter().find(|e| e.path == "hello.txt").unwrap();
        assert_eq!(hello.size, 15);
        assert!(!hello.is_directory);

        let world = result
            .entries
            .iter()
            .find(|e| e.path == "data/world.txt")
            .unwrap();
        assert_eq!(world.size, 10);
        assert!(!world.is_directory);

        let dir = result
            .entries
            .iter()
            .find(|e| e.path == "empty_dir/")
            .unwrap();
        assert!(dir.is_directory);
    }

    #[test]
    fn test_zip_slip_rejection() {
        assert!(!is_safe_path("../../etc/passwd"));
        assert!(!is_safe_path("../secret.txt"));
        assert!(!is_safe_path("/etc/passwd"));
        assert!(!is_safe_path("foo/../../bar"));

        assert!(is_safe_path("safe/path/file.txt"));
        assert!(is_safe_path("hello.txt"));
        assert!(is_safe_path("data/nested/deep/file.rs"));
    }

    #[test]
    fn test_extract_text_entry() {
        let zip_file = create_test_zip();
        let output_dir = tempfile::tempdir().unwrap();

        let result = extract_entry(zip_file.path(), "hello.txt", output_dir.path()).unwrap();

        assert_eq!(result.size, 15);
        assert_eq!(result.content.as_deref(), Some("Hello from zip!"));
        assert!(result.path.exists());

        let on_disk = std::fs::read_to_string(&result.path).unwrap();
        assert_eq!(on_disk, "Hello from zip!");
    }

    #[test]
    fn test_extract_nested_entry() {
        let zip_file = create_test_zip();
        let output_dir = tempfile::tempdir().unwrap();

        let result =
            extract_entry(zip_file.path(), "data/world.txt", output_dir.path()).unwrap();

        assert_eq!(result.size, 10);
        assert_eq!(result.content.as_deref(), Some("World data"));
        assert!(result.path.exists());
        assert!(output_dir.path().join("data").is_dir());
    }

    #[test]
    fn test_extract_rejects_traversal() {
        let zip_file = create_test_zip();
        let output_dir = tempfile::tempdir().unwrap();

        let result = extract_entry(zip_file.path(), "../../etc/passwd", output_dir.path());
        assert!(result.is_err());

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unsafe entry path rejected"));
    }

    #[test]
    fn test_extract_nonexistent_entry() {
        let zip_file = create_test_zip();
        let output_dir = tempfile::tempdir().unwrap();

        let result = extract_entry(zip_file.path(), "nonexistent.txt", output_dir.path());
        assert!(result.is_err());

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Entry not found"));
    }

    #[test]
    fn test_unsupported_format() {
        let f = tempfile::NamedTempFile::with_suffix(".rar").unwrap();
        let result = list_archive(f.path());
        assert!(result.is_err());

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unsupported archive format"));
    }
}
