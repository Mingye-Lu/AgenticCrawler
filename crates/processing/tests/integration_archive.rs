use std::io::Write;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn ensure_sample_zip() -> PathBuf {
    let path = fixtures_dir().join("sample.zip");
    if !path.exists() {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("readme.txt", opts).unwrap();
        zip.write_all(b"This is a sample readme file.").unwrap();

        zip.start_file("data/numbers.txt", opts).unwrap();
        zip.write_all(b"1\n2\n3\n4\n5").unwrap();

        zip.start_file("notes.txt", opts).unwrap();
        zip.write_all(b"Some notes here.").unwrap();

        zip.finish().unwrap();
    }
    path
}

#[test]
fn list_sample_zip() {
    use acrawl_processing::archive::list_archive;

    let path = ensure_sample_zip();
    let result = list_archive(&path).unwrap();

    assert_eq!(result.format, "zip");
    assert_eq!(result.total_files, 3);
    let names: Vec<&str> = result.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(names.contains(&"readme.txt"));
    assert!(names.contains(&"data/numbers.txt"));
    assert!(names.contains(&"notes.txt"));
}

#[test]
fn list_zip_entry_sizes() {
    use acrawl_processing::archive::list_archive;

    let path = ensure_sample_zip();
    let result = list_archive(&path).unwrap();

    let readme = result
        .entries
        .iter()
        .find(|e| e.path == "readme.txt")
        .unwrap();
    assert_eq!(readme.size, 29);
    assert!(!readme.is_directory);
}

#[test]
fn extract_text_entry_from_zip() {
    use acrawl_processing::archive::extract_entry;

    let path = ensure_sample_zip();
    let out_dir = std::env::temp_dir().join(format!("acrawl_int_test_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();

    let extracted = extract_entry(&path, "readme.txt", &out_dir).unwrap();
    assert_eq!(extracted.size, 29);
    assert!(extracted.content.is_some());
    assert!(extracted.content.unwrap().contains("sample"));

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn extract_nested_entry_from_zip() {
    use acrawl_processing::archive::extract_entry;

    let path = ensure_sample_zip();
    let out_dir = std::env::temp_dir().join(format!("acrawl_int_test_nested_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();

    let extracted = extract_entry(&path, "data/numbers.txt", &out_dir).unwrap();
    assert_eq!(extracted.size, 9);
    assert!(extracted.content.is_some());
    assert!(extracted.content.unwrap().contains("1\n2\n3"));

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn extract_nonexistent_entry_errors() {
    use acrawl_processing::archive::extract_entry;

    let path = ensure_sample_zip();
    let out_dir = std::env::temp_dir().join(format!("acrawl_int_test_noent_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();

    let result = extract_entry(&path, "does_not_exist.txt", &out_dir);
    assert!(result.is_err());

    let _ = std::fs::remove_dir_all(&out_dir);
}
