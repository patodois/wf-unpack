//! The `version` command on folders that are not a supported build. All inputs are synthetic and need no
//! game files, so this runs in CI.

use std::path::PathBuf;
use std::process::Command;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wf-unpack-version-cli-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn toc_header(version: u32) -> Vec<u8> {
    let mut raw = 0x1867_C64Eu32.to_le_bytes().to_vec();
    raw.extend(version.to_le_bytes());
    raw
}

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wf-unpack"))
        .arg("version")
        .args(args)
        .arg("--game")
        .arg(dir)
        .output()
        .unwrap()
}

#[test]
fn a_toc_this_reader_does_not_read_is_unsupported_with_a_clear_error() {
    let dir = folder("toc21");
    std::fs::write(dir.join("H.Misc.toc"), toc_header(21)).unwrap();
    for args in [&[][..], &["--strict"][..]] {
        let out = run(&dir, args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stdout.contains("support:          unsupported"), "{stdout}");
        assert!(
            stdout.contains(".toc version 21 in 1 package(s)"),
            "{stdout}"
        );
        assert!(
            stderr.contains("this build of Warframe is not supported by this reader"),
            "{stderr}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_empty_folder_is_not_a_build() {
    let dir = folder("empty");
    let out = run(&dir, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("no .toc files"));
    std::fs::remove_dir_all(&dir).unwrap();
}
