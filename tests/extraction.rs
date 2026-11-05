use std::{fs, path::PathBuf, process::Command};
fn fixture(name: &str, claimed: u32, path: &str) -> (PathBuf, PathBuf) {
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
    (root, out)
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
fn extraction_does_not_follow_existing_symlinks_outside_output() {
    for ancestor in [false, true] {
        let (r, o) = fixture(
            if ancestor { "ancestor" } else { "final" },
            3,
            if ancestor {
                "nested/file.txt"
            } else {
                "file.txt"
            },
        );
        let outside = r.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("file.txt");
        fs::write(&target, b"old").unwrap();
        std::os::unix::fs::symlink(
            if ancestor { &outside } else { &target },
            o.join(if ancestor { "nested" } else { "file.txt" }),
        )
        .unwrap();
        let result = extract(&r, &o);
        assert_eq!(
            fs::read(&target).unwrap(),
            b"old",
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.status.success());
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
