//! Updates without Windows: which GitHub release is newer, which of its
//! files may be downloaded, whether the zip is signed with our key, and
//! unpacking it safely (1.8's newer_release / release_asset / _unpack,
//! plus the signature check 2.0 adds).

use std::path::Path;

use serde_json::Value;

pub const REPO: &str = "vladZah403/pc-widget";
pub const LATEST_API: &str = "https://api.github.com/repos/vladZah403/pc-widget/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/vladZah403/pc-widget/releases";
pub const ZIP_NAME: &str = "PC-Monitor.zip";
/// The zip's minisign signature, made with the key on Vlad's PC.
pub const SIG_NAME: &str = "PC-Monitor.zip.minisig";
/// The public half of the release key (minisign); the secret half is only
/// on Vlad's PC. A zip without a valid signature is never installed.
pub const PUBLIC_KEY: &str = "RWRl4jRnn5RGbck2PpZTPtvrKF4aLJK+e3zfaGL7Q9fOpxoKfpBN+X5T"; // key id 6D46949F6734E265
const MAX_ZIP: u64 = 200 << 20;
const MAX_SIG: u64 = 4096;
/// The program unpacks to well under these: a far bigger archive is not ours.
const UNPACK_MAX_FILES: usize = 5000;
const UNPACK_MAX_BYTES: u64 = 500 << 20;
const ROOT: &str = "PC-Monitor/";

/// "v1.10", "2.0.1" -> [1, 10] / [2, 0, 1]; None unless 1 to 4 plain numbers.
pub fn parse_version(text: &str) -> Option<Vec<u32>> {
    let t = text.trim();
    let t = t.strip_prefix('v').unwrap_or(t);
    let parts: Vec<&str> = t.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    parts
        .iter()
        .map(|p| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok()).flatten())
        .collect()
}

/// Versions compared as 1.8 does, with missing parts as zeros ("2.0" == "2.0.0").
fn newer(got: &[u32], have: &[u32]) -> bool {
    let at = |v: &[u32], i: usize| v.get(i).copied().unwrap_or(0);
    (0..4).map(|i| (at(got, i), at(have, i))).find(|(g, h)| g != h).is_some_and(|(g, h)| g > h)
}

/// A file of the release that may be downloaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// A release newer than us.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "2.0.1"
    pub version: String,
    /// Its page (only ever one of ours).
    pub page: String,
    /// PC-Monitor.zip and its signature; None: only the page is offered.
    pub zip: Option<Asset>,
    pub sig: Option<Asset>,
}

impl Release {
    /// Everything for an update in place is there.
    pub fn installable(&self) -> bool {
        self.zip.is_some() && self.sig.is_some()
    }
}

fn is_tag_page(url: &str) -> bool {
    url.strip_prefix(RELEASES_URL)
        .and_then(|r| r.strip_prefix("/tag/"))
        .is_some_and(|tag| tag.starts_with('v') && parse_version(tag).is_some())
}

/// A release file named `name` from GitHub's JSON, or None when it's missing
/// or anything about it looks off.
fn asset(info: &Value, name: &str, max: u64) -> Option<Asset> {
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    info.get("assets")?.as_array()?.iter().find_map(|a| {
        if a.get("name")?.as_str()? != name {
            return None;
        }
        let url = a.get("browser_download_url")?.as_str()?;
        let digest = a.get("digest")?.as_str()?.strip_prefix("sha256:")?;
        let size = a.get("size")?.as_u64()?;
        let hex =
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        (url.starts_with(&prefix) && hex && size > 0 && size < max).then(|| Asset {
            url: url.to_string(),
            sha256: digest.to_string(),
            size,
        })
    })
}

/// The release in GitHub's "latest release" JSON, if it is newer than `current`.
pub fn newer_release(info: &Value, current: &str) -> Option<Release> {
    if info.get("draft").and_then(Value::as_bool) == Some(true)
        || info.get("prerelease").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let got = parse_version(info.get("tag_name")?.as_str()?)?;
    // a beta ("2.0.0-beta.1") is older than its release ("2.0.0")
    let (have, beta) = current.split_once('-').map_or((current, false), |(n, _)| (n, true));
    let have = parse_version(have)?;
    let same = !newer(&got, &have) && !newer(&have, &got);
    if !(newer(&got, &have) || (beta && same)) {
        return None;
    }
    let page = info
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| is_tag_page(u))
        .map_or_else(|| format!("{RELEASES_URL}/latest"), str::to_string);
    Some(Release {
        version: got.iter().map(u32::to_string).collect::<Vec<_>>().join("."),
        page,
        zip: asset(info, ZIP_NAME, MAX_ZIP),
        sig: asset(info, SIG_NAME, MAX_SIG),
    })
}

/// Is `zip` signed (minisign) with the key whose public half is `public_key`?
pub fn verify_signature(zip: &[u8], minisig: &[u8], public_key: &str) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|e| format!("key: {e}"))?;
    let text = std::str::from_utf8(minisig).map_err(|_| "the signature is not text".to_string())?;
    let sig = minisign_verify::Signature::decode(text).map_err(|e| format!("signature: {e}"))?;
    key.verify(zip, &sig, false).map_err(|e| format!("signature: {e}"))
}

/// The release's zip and signature, and that the signature is for this
/// version: its trusted comment (signed too) is "PC Monitor <version>", as
/// the release flow writes it. So an older signed zip can't be passed off
/// as the new one (a downgrade to a version with a known flaw).
pub fn verify_release(zip: &[u8], minisig: &[u8], public_key: &str, version: &str) -> Result<(), String> {
    verify_signature(zip, minisig, public_key)?;
    let text = std::str::from_utf8(minisig).map_err(|_| "the signature is not text".to_string())?;
    let sig = minisign_verify::Signature::decode(text).map_err(|e| format!("signature: {e}"))?;
    let want = format!("PC Monitor {version}");
    if sig.trusted_comment().trim() != want {
        return Err(format!("the signature is for \"{}\", not \"{want}\"", sig.trusted_comment().trim()));
    }
    Ok(())
}

/// Unpack the files under PC-Monitor/ of the release zip into `staging`
/// (emptied first), never anywhere outside it. It must hold PC-Monitor.exe
/// and _internal. The setup's uninstaller (unins000.exe/.dat) in `app_dir`
/// comes along, or "Apps & features" couldn't remove the program any more.
pub fn unpack(zip: &[u8], staging: &Path, app_dir: &Path) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| err(&e))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        total += archive.by_index_raw(i).map_err(|e| err(&e))?.size();
    }
    if archive.len() > UNPACK_MAX_FILES || total > UNPACK_MAX_BYTES {
        return Err("no program in the archive".into());
    }
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(|e| err(&e))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| err(&e))?;
        let name = file.name().map_err(|e| err(&e))?.replace('\\', "/");
        let Some(rel) = name.strip_prefix(ROOT) else { continue };
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        // only plain names: no "..", no drive letters, no absolute parts
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':')) {
            return Err(format!("bad path in the archive: {name}"));
        }
        let target = parts.iter().fold(staging.to_path_buf(), |p, part| p.join(part));
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err(&e))?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| err(&e))?;
        std::io::copy(&mut file, &mut out).map_err(|e| err(&e))?;
    }
    if !(staging.join("PC-Monitor.exe").is_file() && staging.join("_internal").is_dir()) {
        return Err("no program in the archive".into());
    }
    if let Ok(entries) = std::fs::read_dir(app_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("unins") && e.path().is_file() {
                std::fs::copy(e.path(), staging.join(e.file_name())).map_err(|e| err(&e))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn release_json(tag: &str, assets: Value) -> Value {
        serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "html_url": format!("{RELEASES_URL}/tag/{tag}"), "assets": assets,
        })
    }

    fn asset_json(name: &str, size: u64) -> Value {
        serde_json::json!({
            "name": name, "size": size, "digest": format!("sha256:{}", "ab".repeat(32)),
            "browser_download_url": format!("https://github.com/{REPO}/releases/download/v2.0.1/{name}"),
        })
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.10"), Some(vec![1, 10]));
        assert_eq!(parse_version("2.0.1"), Some(vec![2, 0, 1]));
        assert_eq!(parse_version("v2.0-beta"), None);
        assert_eq!(parse_version("1.2.3.4.5"), None);
        assert!(newer(&[2, 0, 1], &[2, 0, 0]) && newer(&[1, 10], &[1, 9, 9]));
        assert!(!newer(&[2, 0], &[2, 0, 0]) && !newer(&[1, 8, 1], &[2, 0, 0]));
    }

    #[test]
    fn picks_a_newer_release_with_its_files() {
        let info = release_json(
            "v2.0.1",
            serde_json::json!([asset_json(ZIP_NAME, 7_000_000), asset_json(SIG_NAME, 300)]),
        );
        let r = newer_release(&info, "2.0.0").expect("newer");
        assert_eq!(r.version, "2.0.1");
        assert_eq!(r.page, format!("{RELEASES_URL}/tag/v2.0.1"));
        assert!(r.installable());
        assert_eq!(r.zip.unwrap().size, 7_000_000);
        assert!(newer_release(&info, "2.0.1").is_none(), "the same version is not newer");
        assert!(newer_release(&info, "2.0.1-beta.2").is_some(), "a beta gets its release");
        assert!(newer_release(&info, "2.0.2-beta.1").is_none());
    }

    #[test]
    fn distrusts_odd_releases() {
        let mut info = release_json("v2.1", serde_json::json!([asset_json(ZIP_NAME, 7_000_000)]));
        info["prerelease"] = true.into();
        assert!(newer_release(&info, "2.0.0").is_none(), "pre-releases are for testers");
        info["prerelease"] = false.into();
        info["html_url"] = "https://evil.example/x".into();
        let r = newer_release(&info, "2.0.0").unwrap();
        assert_eq!(r.page, format!("{RELEASES_URL}/latest"), "only our own page is opened");
        assert!(!r.installable(), "no signature: only the page");
        let mut bad = asset_json(ZIP_NAME, 7_000_000);
        bad["browser_download_url"] = "https://evil.example/PC-Monitor.zip".into();
        let info = release_json("v2.1", serde_json::json!([bad]));
        assert!(newer_release(&info, "2.0.0").unwrap().zip.is_none());
    }

    #[test]
    fn signatures() {
        // minisign-verify's own prehashed test vector: "test" signed by its key
        let key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let sig = [
            "untrusted comment: signature from minisign secret key",
            "RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=",
            "trusted comment: timestamp:1556193335\tfile:test",
            "y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==",
        ]
        .join("\n");
        assert_eq!(verify_signature(b"test", sig.as_bytes(), key), Ok(()));
        assert!(verify_signature(b"Test", sig.as_bytes(), key).is_err(), "another file");
        assert!(verify_signature(b"test", b"garbage", key).is_err());
        assert!(verify_signature(b"test", sig.as_bytes(), PUBLIC_KEY).is_err(), "not our key");
    }

    #[test]
    fn our_release_key() {
        // signed on Vlad's PC with the secret key (rsign sign, prehashed): PUBLIC_KEY is its other half
        let file = include_bytes!("../tests/fixtures/release-key-check.txt");
        let sig = include_bytes!("../tests/fixtures/release-key-check.txt.minisig");
        assert_eq!(verify_signature(file, sig, PUBLIC_KEY), Ok(()));
        let mut changed = file.to_vec();
        changed[0] ^= 1;
        assert!(verify_signature(&changed, sig, PUBLIC_KEY).is_err());
        // its trusted comment is "release key check": no release version matches it
        assert!(verify_release(file, sig, PUBLIC_KEY, "2.0.1").is_err());
    }

    #[test]
    fn the_signature_names_the_version() {
        // the real 2.0.0 signature: right for 2.0.0, refused for any other version
        let sig = include_bytes!("../../../signatures/v2.0.0.minisig");
        let text = std::str::from_utf8(sig).unwrap();
        let decoded = minisign_verify::Signature::decode(text).unwrap();
        assert_eq!(decoded.trusted_comment(), "PC Monitor 2.0.0");
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn unpacks_only_the_program() {
        let root = std::env::temp_dir().join(format!("pcm-unpack-{}", std::process::id()));
        let (app, staging) = (root.join("app"), root.join("app.new"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("unins000.exe"), b"u").unwrap();
        let good = zip_of(&[
            ("PC-Monitor/PC-Monitor.exe", b"exe"),
            ("PC-Monitor/_internal/pcm-sensors.exe", b"helper"),
            ("README.txt", b"skipped"),
        ]);
        unpack(&good, &staging, &app).unwrap();
        assert_eq!(std::fs::read(staging.join("_internal").join("pcm-sensors.exe")).unwrap(), b"helper");
        assert!(staging.join("unins000.exe").is_file(), "the uninstaller comes along");
        assert!(!staging.join("README.txt").exists());
        let evil = zip_of(&[("PC-Monitor/../evil.exe", b"x"), ("PC-Monitor/PC-Monitor.exe", b"exe")]);
        assert!(unpack(&evil, &staging, &app).is_err());
        assert!(!root.join("evil.exe").exists());
        let empty = zip_of(&[("PC-Monitor/readme.txt", b"x")]);
        assert!(unpack(&empty, &staging, &app).is_err(), "no exe, no _internal");
        let _ = std::fs::remove_dir_all(&root);
    }
}
