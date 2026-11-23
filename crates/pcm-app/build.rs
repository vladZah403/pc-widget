//! PC-Monitor.exe's resources: the logo (assets/app.ico, drawn by
//! `cargo run -p pcm-render --example logo`) as icon 1, which Explorer, the
//! shortcuts and the windows show, and the version for the file's
//! Properties and Task Manager.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("../../assets/app.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // rc.exe reads the script in the ANSI code page: everything it names
    // goes to OUT_DIR (no Cyrillic folder names in it)
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::copy(&ico, out.join("app.ico")).expect("assets/app.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut nums: Vec<u16> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).take(3).collect();
    nums.resize(4, 0);
    let commas = nums.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let rc = format!(
        r#"1 ICON "app.ico"

1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "vladZah403"
      VALUE "FileDescription", "PC Monitor"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "PC-Monitor"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "PC-Monitor.exe"
      VALUE "ProductName", "PC Monitor"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let script = out.join("app.rc");
    std::fs::write(&script, rc).expect("app.rc");
    embed_resource::compile(&script, embed_resource::NONE).manifest_optional().expect("resources");
}
