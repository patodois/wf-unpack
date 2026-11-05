use std::{fs, path::PathBuf, process::Command};
struct Temp(PathBuf);
impl std::ops::Deref for Temp {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str, claimed: u32, path: &str) -> (Temp, PathBuf) {
    let root = std::env::temp_dir().join(format!("wf-extraction-{name}-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut b = vec![0; 104];
    b[..4].copy_from_slice(&0x1867c64eu32.to_le_bytes());
    b[4..8].copy_from_slice(&20u32.to_le_bytes());
    b[24..28].copy_from_slice(&claimed.to_le_bytes());
    b[28..32].copy_from_slice(&claimed.to_le_bytes());
    b[40..40 + path.len()].copy_from_slice(path.as_bytes());
    fs::write(root.join("Small.toc"), b).unwrap();
    fs::write(root.join("Small.cache"), b"new").unwrap();
    let out = root.join("out");
    fs::create_dir_all(&out).unwrap();
    (Temp(root), out)
}
fn extract(root: &std::path::Path, out: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wf-unpack"))
        .arg("extract")
        .arg(root.join("Small.toc"))
        .arg("**")
        .arg("-o")
        .arg(out)
        .output()
        .unwrap()
}
#[test]
fn ordinary_extraction_writes_selected_content() {
    let (r, o) = fixture("ordinary", 3, "file.txt");
    let result = extract(&r, &o);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(o.join("file.txt")).unwrap(), b"new");
}
#[test]
#[cfg(unix)]
fn final_symlink_is_refused_and_other_entries_continue() {
    let (r, o) = fixture("final", 3, "file.txt");
    let target = r.join("target.txt");
    fs::write(&target, b"old").unwrap();
    std::os::unix::fs::symlink(&target, o.join("file.txt")).unwrap();
    let mut toc = fs::read(r.join("Small.toc")).unwrap();
    let mut entry = vec![0; 96];
    entry[16..20].copy_from_slice(&3u32.to_le_bytes());
    entry[20..24].copy_from_slice(&3u32.to_le_bytes());
    entry[32..40].copy_from_slice(b"safe.txt");
    toc.extend_from_slice(&entry);
    fs::write(r.join("Small.toc"), toc).unwrap();
    let result = extract(&r, &o);
    assert!(!result.status.success());
    assert_eq!(fs::read(target).unwrap(), b"old");
    assert_eq!(fs::read(o.join("safe.txt")).unwrap(), b"new");
    assert!(String::from_utf8_lossy(&result.stderr).contains("(1 failed)"));
}

#[test]
#[cfg(unix)]
fn ancestor_symlinks_allow_absolute_targets_inside_and_outside_output() {
    for inside in [false, true] {
        let (r, o) = fixture(
            if inside { "inside" } else { "outside" },
            3,
            "nested/file.txt",
        );
        let target = if inside {
            o.join("target")
        } else {
            r.join("target")
        };
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, o.join("nested")).unwrap();
        let result = extract(&r, &o);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read(target.join("file.txt")).unwrap(), b"new");
    }
}

#[test]
fn incomplete_extraction_returns_failure() {
    let (r, o) = fixture("short", 5, "file.txt");
    let result = extract(&r, &o);
    assert!(
        !result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!o.join("file.txt").exists());
}
