use super::*;

#[test]
fn writes_under_the_root_and_returns_the_relative_path_to_store() {
    let dir = tempfile::tempdir().unwrap();
    let relative = write_image(dir.path(), 12, 3, StoredFormat::Jpeg, b"jpeg bytes").unwrap();
    assert_eq!(relative, "images/12/3.jpg");
    let on_disk = fs::read(dir.path().join("images").join("12").join("3.jpg")).unwrap();
    assert_eq!(on_disk, b"jpeg bytes");
    assert_eq!(
        resolve(dir.path(), &relative).unwrap(),
        dir.path().join("images").join("12").join("3.jpg")
    );
}

#[test]
fn rewriting_replaces_the_file_atomically_and_leaves_no_temporary_behind() {
    let dir = tempfile::tempdir().unwrap();
    write_image(dir.path(), 1, 0, StoredFormat::Png, b"first").unwrap();
    write_image(dir.path(), 1, 0, StoredFormat::Png, b"second, longer").unwrap();
    let folder = dir.path().join("images").join("1");
    assert_eq!(fs::read(folder.join("0.png")).unwrap(), b"second, longer");
    let names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["0.png"], "no .tmp file may remain");
}

#[test]
fn every_format_gets_its_fixed_extension() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        write_image(dir.path(), 1, 0, StoredFormat::Png, b"x").unwrap(),
        "images/1/0.png"
    );
    assert_eq!(
        write_image(dir.path(), 1, 1, StoredFormat::WebP, b"x").unwrap(),
        "images/1/1.webp"
    );
    assert_eq!(
        write_image(dir.path(), 1, 2, StoredFormat::Jpeg, b"x").unwrap(),
        "images/1/2.jpg"
    );
}

#[test]
fn stored_paths_that_could_escape_the_cache_are_refused() {
    let root = Path::new("cache-root");
    for hostile in [
        "",
        "../secret.txt",
        "images/../../secret.txt",
        "images/1/../../../secret.txt",
        "/etc/passwd",
        "\\windows\\system32",
        "C:\\Users\\x.jpg",
        "C:/Users/x.jpg",
        "images//1/0.jpg",
        "images/./1/0.jpg",
        "images/1/",
        "images/1/0.jpg/..",
        "images\\1\\0.jpg",
        "images/1/0\0.jpg",
        "\\\\server\\share\\x.jpg",
        "..",
        ".",
    ] {
        assert_eq!(resolve(root, hostile), None, "{hostile:?} must be refused");
    }
}

#[test]
fn ordinary_relative_paths_resolve_under_the_root() {
    let root = Path::new("cache-root");
    assert_eq!(
        resolve(root, "images/7/0.jpg").unwrap(),
        root.join("images").join("7").join("0.jpg")
    );
    assert!(resolve(root, "images/7/photo.final.webp").is_some());
}
