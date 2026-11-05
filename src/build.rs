//! Which game build an install is on, and whether this reader supports it.
//!
//! The readers know two format numbers: the `.toc` version ([`toc::VERSION`], 20) and the version of
//! `/Packages.bin` ([`packages::MIN_VERSION`] and later). Neither says which *game build* wrote the files, so
//! this module also reads, when they are there:
//!
//! - the **file version** that `Warframe.x64.exe` carries in its version resource (a string such as
//!   `2026.10.06.11.44`; the same shape as the build label that private-server projects use, but this reader
//!   has not confirmed it is the same value), from the PE `.rsrc` section, by seeking: the 45 MB executable
//!   is not read whole;
//! - the **Steam build id** and update time from `steamapps/appmanifest_230410.acf`, next to the install or
//!   in the Steam library that holds it.
//!
//! A build is [`Support::Verified`] when one of those identities is in [`VERIFIED`] (an install this reader's
//! checks were run on), [`Support::Readable`] when its formats are in range but it is a build nobody here has
//! run the checks on, and [`Support::Unsupported`] when a format number is outside what the parsers accept.
//!
//! [`Strictness`] decides what a caller does with that. [`Strictness::Lenient`] is how the readers have
//! always behaved: each package is checked when it is opened and nothing else is asked. [`Strictness::Strict`]
//! checks everything up front and refuses with one error that names the build: an unsupported format
//! anywhere, and a `Packages.bin` layout newer than [`PACKAGES_MAX_VERIFIED`], which the parser would
//! otherwise read with the layout of version 46 and hope for the best.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cache::Package;
use crate::{packages, toc};

/// The Steam application id of Warframe.
pub const STEAM_APP_ID: u32 = 230_410;

/// The highest `Packages.bin` version this reader has been run against. `Strict` refuses a higher one.
pub const PACKAGES_MAX_VERIFIED: u32 = 46;

/// An install the checks of the README were run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedBuild {
    /// Steam's build id (`buildid` in the appmanifest)
    pub steam_build_id: u64,
    /// `FileVersion` of `Warframe.x64.exe`
    pub file_version: &'static str,
    /// The day Steam finished the update (`LastUpdated`)
    pub date: &'static str,
}

/// The builds the README's check table was produced on.
pub const VERIFIED: &[VerifiedBuild] = &[VerifiedBuild {
    steam_build_id: 25_757_057,
    file_version: "2026.10.06.11.44",
    date: "2026-10-22",
}];

/// How hard a caller wants the build checked before it reads anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strictness {
    /// The default of every reader: formats are checked as packages are opened.
    #[default]
    Lenient,
    /// Refuse up front, with one clear error, a build whose formats this reader does not support.
    Strict,
}

/// How far this reader supports a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// A build in [`VERIFIED`].
    Verified,
    /// Formats in range, but not a build the checks were run on.
    Readable,
    /// A format number outside what the parsers accept (or the files are not there).
    Unsupported,
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Support::Verified => "verified",
            Support::Readable => "readable, not verified",
            Support::Unsupported => "unsupported",
        })
    }
}

/// What the install's Steam manifest says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamInfo {
    pub build_id: u64,
    /// Unix time of `LastUpdated`
    pub last_updated: Option<u64>,
}

/// What was found out about an install. Every field is read from the files; a missing source is `None`,
/// never a guess.
#[derive(Debug, Clone)]
pub struct GameBuild {
    pub game: PathBuf,
    /// `.toc` files by their version number
    pub toc_versions: BTreeMap<u32, usize>,
    /// Files named `.toc` that are not one (wrong magic, too short), by file name
    pub bad_tocs: Vec<String>,
    /// The version of `/Packages.bin`, or why it could not be read
    pub packages_version: std::result::Result<u32, String>,
    /// `FileVersion` of `Warframe.x64.exe` beside the `Cache.Windows` folder
    pub file_version: Option<String>,
    /// `ProductVersion` of the same, when it differs from the file version
    pub product_version: Option<String>,
    pub steam: Option<SteamInfo>,
}

impl GameBuild {
    /// Reads the build of the `Cache.Windows` folder `game`. Reads 8 bytes of every `.toc`, the head of
    /// `/Packages.bin` (the whole entry is decompressed, a few tens of MB) and the version resource of the
    /// executable; nothing else.
    pub fn inspect(game: &Path) -> Result<Self> {
        let scan = scan_tocs(game)?;
        let packages_version = packages_version(game).map_err(|e| format!("{e:#}"));
        let install = game.parent();
        let (file_version, product_version) = install
            .map(|dir| dir.join("Warframe.x64.exe"))
            .filter(|exe| exe.is_file())
            .and_then(|exe| File::open(exe).ok())
            .and_then(|mut f| pe_versions(&mut f).ok())
            .map_or((None, None), |(file, product)| {
                let product = (product != file).then_some(product);
                (Some(file), product)
            });
        let steam = install.and_then(steam_info);
        Ok(GameBuild {
            game: game.to_path_buf(),
            toc_versions: scan.versions,
            bad_tocs: scan.bad,
            packages_version,
            file_version,
            product_version,
            steam,
        })
    }

    /// Why this build is unsupported; empty when it is not.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.toc_versions.is_empty() && self.bad_tocs.is_empty() {
            out.push(format!("no .toc files in {}", self.game.display()));
        }
        for (v, n) in &self.toc_versions {
            if *v != toc::VERSION {
                out.push(format!(
                    ".toc version {v} in {n} package(s); this reader reads version {} only",
                    toc::VERSION
                ));
            }
        }
        if !self.bad_tocs.is_empty() {
            out.push(format!(
                "not .toc files despite the name: {}",
                list_some(&self.bad_tocs)
            ));
        }
        match &self.packages_version {
            Ok(v) if *v < packages::MIN_VERSION => out.push(format!(
                "Packages.bin version {v}; this reader reads {} and later",
                packages::MIN_VERSION
            )),
            Ok(_) => {}
            Err(e) => out.push(format!("Packages.bin cannot be read: {e}")),
        }
        out
    }

    /// What `Strict` refuses on top of [`GameBuild::problems`]: a layout newer than any this reader ran on.
    pub fn strict_problems(&self) -> Vec<String> {
        match &self.packages_version {
            Ok(v) if *v > PACKAGES_MAX_VERIFIED => vec![format!(
                "Packages.bin version {v} is newer than the highest this reader ran on ({PACKAGES_MAX_VERIFIED}); \
                 its layout may have changed"
            )],
            _ => Vec::new(),
        }
    }

    /// The build in [`VERIFIED`] this install is, by its executable's file version or its Steam build id.
    pub fn verified_as(&self) -> Option<&'static VerifiedBuild> {
        VERIFIED.iter().find(|v| {
            self.file_version.as_deref() == Some(v.file_version)
                || self
                    .steam
                    .as_ref()
                    .is_some_and(|s| s.build_id == v.steam_build_id)
        })
    }

    pub fn support(&self) -> Support {
        if !self.problems().is_empty() {
            Support::Unsupported
        } else if self.verified_as().is_some() {
            Support::Verified
        } else {
            Support::Readable
        }
    }

    /// One line naming the build, for errors: `file version 2026.10.06.11.44, Steam build 25757057`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        match &self.file_version {
            Some(v) => parts.push(format!("file version {v}")),
            None => {
                parts.push("file version unknown (no Warframe.x64.exe beside the folder)".into())
            }
        }
        if let Some(s) = &self.steam {
            parts.push(format!("Steam build {}", s.build_id));
        }
        for (v, n) in &self.toc_versions {
            parts.push(format!(".toc v{v} x{n}"));
        }
        if let Ok(v) = &self.packages_version {
            parts.push(format!("Packages.bin v{v}"));
        }
        parts.join(", ")
    }

    /// `Ok` when `strictness` accepts this build, otherwise one error that says what is wrong and which
    /// build it is. `Lenient` refuses what the parsers would refuse anyway; `Strict` also refuses a newer
    /// `Packages.bin` layout.
    pub fn check(&self, strictness: Strictness) -> Result<()> {
        let mut problems = self.problems();
        if strictness == Strictness::Strict {
            problems.extend(self.strict_problems());
        }
        if problems.is_empty() {
            return Ok(());
        }
        bail!(
            "this build of Warframe is not supported by this reader ({}): {}",
            self.describe(),
            problems.join("; ")
        )
    }
}

fn list_some(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
    if names.len() > shown.len() {
        format!(
            "{} and {} more",
            shown.join(", "),
            names.len() - shown.len()
        )
    } else {
        shown.join(", ")
    }
}

/// What the first 8 bytes of every `.toc` of a folder say.
#[derive(Debug, Default)]
pub struct TocScan {
    pub versions: BTreeMap<u32, usize>,
    pub bad: Vec<String>,
}

/// Reads the magic and version of every `*.toc` in `game` (8 bytes each).
pub fn scan_tocs(game: &Path) -> Result<TocScan> {
    let mut scan = TocScan::default();
    let dir = std::fs::read_dir(game).with_context(|| format!("reading {}", game.display()))?;
    let mut names: Vec<_> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toc"))
        .map(|e| e.path())
        .collect();
    names.sort();
    for path in names {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut head = [0u8; 8];
        let ok = File::open(&path)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok();
        let magic = u32::from_le_bytes(head[0..4].try_into().unwrap());
        if ok && magic == toc::MAGIC {
            let version = u32::from_le_bytes(head[4..8].try_into().unwrap());
            *scan.versions.entry(version).or_insert(0) += 1;
        } else {
            scan.bad.push(name);
        }
    }
    Ok(scan)
}

/// The version number in the head of a decompressed `Packages.bin`.
pub fn packages_version_of(raw: &[u8]) -> Result<u32> {
    if raw.len() < 24 || u32::from_le_bytes(raw[16..20].try_into().unwrap()) != 20 {
        bail!("not a Packages.bin header");
    }
    Ok(u32::from_le_bytes(raw[20..24].try_into().unwrap()))
}

fn packages_version(game: &Path) -> Result<u32> {
    let mut pkg = Package::open(&game.join("H.Misc.toc"))?;
    let raw = pkg
        .read_path("/Packages.bin")?
        .context("/Packages.bin is not in H.Misc")?;
    packages_version_of(&raw)
}

/// The value of a top-level `"key"  "value"` pair of a Steam `.acf` file (Valve's text format).
pub fn acf_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let mut quoted = line.split('"').skip(1).step_by(2);
        (quoted.next()? == key).then(|| quoted.next()).flatten()
    })
}

/// Parses an appmanifest.
pub fn parse_appmanifest(text: &str) -> Option<SteamInfo> {
    Some(SteamInfo {
        build_id: acf_value(text, "buildid")?.parse().ok()?,
        last_updated: acf_value(text, "LastUpdated").and_then(|v| v.parse().ok()),
    })
}

/// Looks for the manifest where Steam keeps it: `steamapps/` inside the install folder (a copied install),
/// or the `steamapps/` two levels up (`steamapps/common/Warframe`).
fn steam_info(install: &Path) -> Option<SteamInfo> {
    let name = format!("appmanifest_{STEAM_APP_ID}.acf");
    [
        install.join("steamapps").join(&name),
        install.join("..").join("..").join(&name),
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .and_then(|t| parse_appmanifest(&t))
}

/// `YYYY-MM-DD` of a Unix time (UTC).
pub fn unix_date(secs: u64) -> String {
    // Howard Hinnant's civil-from-days
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The most of a `.rsrc` section that is read.
const MAX_RSRC: u64 = 16 << 20;

/// `FileVersion` and `ProductVersion` of a Windows executable, from the strings of its version resource.
/// Seeks to the `.rsrc` section and reads only that.
pub fn pe_versions<R: Read + Seek>(exe: &mut R) -> Result<(String, String)> {
    let mut dos = [0u8; 64];
    exe.read_exact(&mut dos).context("reading the DOS header")?;
    if &dos[0..2] != b"MZ" {
        bail!("not a PE file");
    }
    let pe = u64::from(u32::from_le_bytes(dos[0x3c..0x40].try_into().unwrap()));
    exe.seek(SeekFrom::Start(pe))?;
    let mut coff = [0u8; 24];
    exe.read_exact(&mut coff).context("reading the PE header")?;
    if &coff[0..4] != b"PE\0\0" {
        bail!("no PE signature");
    }
    let sections = usize::from(u16::from_le_bytes(coff[6..8].try_into().unwrap()));
    let optional = u64::from(u16::from_le_bytes(coff[20..22].try_into().unwrap()));
    exe.seek(SeekFrom::Start(pe + 24 + optional))?;
    let mut rsrc = None;
    for _ in 0..sections.min(96) {
        let mut s = [0u8; 40];
        exe.read_exact(&mut s).context("reading a section header")?;
        if s[0..8].starts_with(b".rsrc\0") {
            let size = u64::from(u32::from_le_bytes(s[16..20].try_into().unwrap()));
            let at = u64::from(u32::from_le_bytes(s[20..24].try_into().unwrap()));
            rsrc = Some((at, size));
            break;
        }
    }
    let (at, size) = rsrc.context("no .rsrc section")?;
    if size > MAX_RSRC {
        bail!(".rsrc section of {size} bytes is larger than the {MAX_RSRC} this reads");
    }
    exe.seek(SeekFrom::Start(at))?;
    let mut data = vec![0u8; size as usize];
    exe.read_exact(&mut data).context("reading .rsrc")?;
    let file = utf16_value(&data, "FileVersion").context("no FileVersion string")?;
    let product = utf16_value(&data, "ProductVersion").unwrap_or_else(|| file.clone());
    Ok((file, product))
}

/// The string after `key` in a version resource: the key and the value are NUL-terminated UTF-16, with
/// zero padding between them.
fn utf16_value(data: &[u8], key: &str) -> Option<String> {
    let mut needle: Vec<u8> = key.encode_utf16().flat_map(u16::to_le_bytes).collect();
    needle.extend([0, 0]);
    let mut from = 0;
    while let Some(i) = data[from..].windows(needle.len()).position(|w| w == needle) {
        let mut at = from + i + needle.len();
        // the key starts a string: the unit before it is a structure header, not a letter of a longer key
        // (`OriginalFileVersion` is not `FileVersion`)
        let starts = from + i < 2 || {
            let before = u16::from_le_bytes([data[from + i - 2], data[from + i - 1]]);
            !char::from_u32(u32::from(before)).is_some_and(char::is_alphanumeric)
        };
        if starts {
            while at + 1 < data.len() && data[at] == 0 && data[at + 1] == 0 {
                at += 2;
            }
            let units: Vec<u16> = data[at..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .take(64)
                .collect();
            if !units.is_empty() {
                return String::from_utf16(&units).ok();
            }
        }
        from += i + 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const ACF: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"230410\"\n\t\"LastUpdated\"\t\t\"1792695023\"\n\t\"buildid\"\t\t\"25757057\"\n\t\"TargetBuildID\"\t\t\"99\"\n}\n";

    #[test]
    fn an_appmanifest_gives_the_build_id_not_the_target() {
        let info = parse_appmanifest(ACF).unwrap();
        assert_eq!(info.build_id, 25_757_057);
        assert_eq!(info.last_updated, Some(1_792_695_023));
        assert_eq!(unix_date(1_792_695_023), "2026-10-22");
        assert!(parse_appmanifest("\"AppState\"\n{\n}\n").is_none());
        assert!(parse_appmanifest("\"buildid\"\t\"abc\"").is_none());
    }

    #[test]
    fn dates_around_a_leap_day() {
        assert_eq!(unix_date(0), "1970-01-01");
        assert_eq!(unix_date(951_782_400), "2000-02-29");
        assert_eq!(unix_date(951_868_800), "2000-03-01");
    }

    /// A PE file with one `.rsrc` section that holds a version resource's strings.
    pub(crate) fn fake_pe(file: &str, product: &str) -> Vec<u8> {
        fake_pe_keys(&[
            ("CompanyName", "Digital Extremes"),
            ("FileVersion", file),
            ("ProductVersion", product),
        ])
    }

    fn fake_pe_keys(pairs: &[(&str, &str)]) -> Vec<u8> {
        let utf16 = |s: &str| -> Vec<u8> {
            s.encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let mut rsrc = vec![0xAB; 6];
        for &(k, v) in pairs {
            // length, value length, type, key, padding to 4 bytes, value
            rsrc.extend([0x30, 0, 0x10, 0, 1, 0]);
            rsrc.extend(utf16(k));
            while !rsrc.len().is_multiple_of(4) {
                rsrc.push(0);
            }
            rsrc.extend(utf16(v));
            while !rsrc.len().is_multiple_of(4) {
                rsrc.push(0);
            }
        }
        let mut exe = vec![0u8; 0x40];
        exe[0..2].copy_from_slice(b"MZ");
        exe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        exe.extend(b"PE\0\0");
        let mut coff = [0u8; 20];
        coff[2..4].copy_from_slice(&2u16.to_le_bytes()); // two sections
        exe.extend(coff); // optional header size 0
        let raw_at = (0x40 + 24 + 2 * 40) as u32;
        let mut text = [0u8; 40];
        text[0..5].copy_from_slice(b".text");
        text[16..20].copy_from_slice(&4u32.to_le_bytes());
        text[20..24].copy_from_slice(&0u32.to_le_bytes());
        let mut rs = [0u8; 40];
        rs[0..5].copy_from_slice(b".rsrc");
        rs[16..20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        rs[20..24].copy_from_slice(&raw_at.to_le_bytes());
        exe.extend(text);
        exe.extend(rs);
        exe.extend(&rsrc);
        exe
    }

    #[test]
    fn the_version_resource_of_an_executable_is_read() {
        let exe = fake_pe("2026.10.06.11.44", "2026.10.06.11.45");
        let (file, product) = pe_versions(&mut Cursor::new(exe)).unwrap();
        assert_eq!(
            (file.as_str(), product.as_str()),
            ("2026.10.06.11.44", "2026.10.06.11.45")
        );
        assert!(pe_versions(&mut Cursor::new(b"not an exe".to_vec())).is_err());
        // A PE with no version strings says so
        let mut bare = fake_pe("x", "y");
        let n = bare.len();
        bare[n - 60..].fill(0xCC);
        assert!(pe_versions(&mut Cursor::new(bare)).is_err());
    }

    #[test]
    fn a_key_inside_a_longer_key_is_not_the_key() {
        // "OriginalFileVersion" must not answer for "FileVersion"
        let exe = fake_pe_keys(&[
            ("OriginalFileVersion", "9.9.9.9"),
            ("ProductVersion", "1.2.3.4"),
        ]);
        assert!(pe_versions(&mut Cursor::new(exe)).is_err());
    }

    fn write_toc(dir: &Path, name: &str, version: u32) {
        let mut raw = toc::MAGIC.to_le_bytes().to_vec();
        raw.extend(version.to_le_bytes());
        std::fs::write(dir.join(name), raw).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wf-unpack-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_toc_is_counted_by_version() {
        let dir = temp("scan");
        write_toc(&dir, "H.Misc.toc", 20);
        write_toc(&dir, "B.Misc.toc", 20);
        write_toc(&dir, "F.New.toc", 21);
        std::fs::write(dir.join("junk.toc"), b"xy").unwrap();
        std::fs::write(dir.join("H.Misc.cache"), b"not a toc").unwrap();
        let scan = scan_tocs(&dir).unwrap();
        assert_eq!(scan.versions.get(&20), Some(&2));
        assert_eq!(scan.versions.get(&21), Some(&1));
        assert_eq!(scan.bad, ["junk.toc"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn build(
        toc_versions: &[(u32, usize)],
        packages: std::result::Result<u32, String>,
    ) -> GameBuild {
        GameBuild {
            game: PathBuf::from("Cache.Windows"),
            toc_versions: toc_versions.iter().copied().collect(),
            bad_tocs: Vec::new(),
            packages_version: packages,
            file_version: None,
            product_version: None,
            steam: None,
        }
    }

    #[test]
    fn support_follows_the_formats_and_the_verified_list() {
        let mut b = build(&[(20, 95)], Ok(46));
        assert_eq!(b.support(), Support::Readable);
        b.file_version = Some(VERIFIED[0].file_version.into());
        assert_eq!(b.support(), Support::Verified);
        b.file_version = None;
        b.steam = Some(SteamInfo {
            build_id: VERIFIED[0].steam_build_id,
            last_updated: None,
        });
        assert_eq!(b.support(), Support::Verified);
        // A format outside the parsers' range is unsupported whatever the build id says
        b.toc_versions.insert(21, 3);
        assert_eq!(b.support(), Support::Unsupported);
        assert_eq!(build(&[(20, 1)], Ok(45)).support(), Support::Unsupported);
        assert_eq!(
            build(&[(20, 1)], Err("gone".into())).support(),
            Support::Unsupported
        );
        assert_eq!(build(&[], Ok(46)).support(), Support::Unsupported);
    }

    #[test]
    fn strict_refuses_what_lenient_lets_through() {
        // A Packages.bin newer than any this reader ran on: the parser accepts "46 and later"
        let newer = build(&[(20, 95)], Ok(PACKAGES_MAX_VERIFIED + 1));
        assert_eq!(newer.support(), Support::Readable);
        assert!(newer.check(Strictness::Lenient).is_ok());
        let err = newer.check(Strictness::Strict).unwrap_err().to_string();
        assert!(err.contains("not supported by this reader"), "{err}");
        assert!(
            err.contains(&format!(
                "Packages.bin version {}",
                PACKAGES_MAX_VERIFIED + 1
            )),
            "{err}"
        );
        assert!(
            err.contains("Packages.bin v47"),
            "the error names the build: {err}"
        );
        // The build this reader ran on passes both
        let known = build(&[(20, 95)], Ok(PACKAGES_MAX_VERIFIED));
        assert!(known.check(Strictness::Strict).is_ok());
        // An unsupported .toc is refused in both, with its version in the message
        let old = build(&[(20, 90), (19, 5)], Ok(46));
        for s in [Strictness::Lenient, Strictness::Strict] {
            let err = old.check(s).unwrap_err().to_string();
            assert!(err.contains(".toc version 19 in 5 package(s)"), "{err}");
        }
    }

    /// A folder that is a very small `Cache.Windows`: an `H.Misc` package whose only entry is a
    /// `Packages.bin` of the given version, and other packages that are only a `.toc` header.
    fn fake_install(name: &str, packages_version: u32, others: &[(&str, u32)]) -> PathBuf {
        let dir = temp(name);
        let mut raw = packages::tests::build(&[(
            "/Lotus/A/",
            "Thing",
            "",
            Some("Mesh=/Lotus/A/Thing.fbx\n"),
        )]);
        raw[20..24].copy_from_slice(&packages_version.to_le_bytes());
        let len = raw.len() as i32;
        let toc = toc::tests::toc(&[toc::tests::entry(0, len, len, 0, "Packages.bin")]);
        std::fs::write(dir.join("H.Misc.toc"), toc).unwrap();
        std::fs::write(dir.join("H.Misc.cache"), raw).unwrap();
        for (n, v) in others {
            write_toc(&dir, &format!("{n}.toc"), *v);
        }
        dir
    }

    #[test]
    fn load_is_lenient_and_load_with_strict_refuses_a_newer_packages_bin() {
        use crate::packages::PackagesBin;
        let dir = fake_install("newer", PACKAGES_MAX_VERIFIED + 1, &[]);
        // What every caller got before strict mode existed, and still gets
        assert_eq!(PackagesBin::load(&dir).unwrap().len(), 1);
        assert_eq!(
            PackagesBin::load_with(&dir, Strictness::Lenient)
                .unwrap()
                .len(),
            1
        );
        let err = PackagesBin::load_with(&dir, Strictness::Strict)
            .err()
            .expect("strict accepted it");
        let err = format!("{err:#}");
        assert!(
            err.contains("not supported by this reader") && err.contains("Packages.bin version 47"),
            "{err}"
        );
        // The version this reader ran on passes strict mode
        let known = fake_install("known", PACKAGES_MAX_VERIFIED, &[("B.Misc", 20)]);
        assert_eq!(
            PackagesBin::load_with(&known, Strictness::Strict)
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&known).unwrap();
    }

    #[test]
    fn strict_sees_a_toc_that_lenient_never_opens() {
        use crate::packages::PackagesBin;
        // H.Misc is fine; another package is a format this reader does not read. Lenient only opens H.Misc
        let dir = fake_install("othertoc", PACKAGES_MAX_VERIFIED, &[("F.Texture", 21)]);
        assert!(PackagesBin::load(&dir).is_ok());
        let err = format!(
            "{:#}",
            PackagesBin::load_with(&dir, Strictness::Strict)
                .err()
                .expect("accepted")
        );
        assert!(err.contains(".toc version 21 in 1 package(s)"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(feature = "bridge")]
    #[test]
    fn the_provider_opens_strictly_or_not() {
        use crate::bridge::Warframe;
        let dir = fake_install("provider", PACKAGES_MAX_VERIFIED, &[("F.Texture", 21)]);
        assert!(Warframe::open(&dir).is_ok());
        assert!(Warframe::open_with(&dir, Strictness::Lenient).is_ok());
        let err = Warframe::open_with(&dir, Strictness::Strict)
            .err()
            .expect("accepted");
        assert!(format!("{err:#}").contains(".toc version 21"), "{err:#}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn inspecting_a_folder_reports_what_is_in_it() {
        let dir = fake_install("inspect", PACKAGES_MAX_VERIFIED, &[("B.Misc", 20)]);
        let b = GameBuild::inspect(&dir).unwrap();
        assert_eq!(b.toc_versions.get(&20), Some(&2));
        assert_eq!(b.packages_version, Ok(PACKAGES_MAX_VERIFIED));
        // No executable and no manifest next to a temp folder: unknown, not guessed
        assert_eq!((b.file_version.clone(), b.steam.clone()), (None, None));
        assert_eq!(b.support(), Support::Readable);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_packages_bin_header_gives_its_version() {
        let mut raw = vec![0u8; 16];
        raw.extend(20u32.to_le_bytes());
        raw.extend(46u32.to_le_bytes());
        assert_eq!(packages_version_of(&raw).unwrap(), 46);
        assert!(packages_version_of(&raw[..20]).is_err());
        raw[16] = 7;
        assert!(packages_version_of(&raw).is_err());
    }
}
