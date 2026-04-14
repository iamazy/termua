use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/logo/termua.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR"));
    let icon_path = manifest_dir.join("../assets/logo/termua.ico");
    let rc_path = out_dir.join("termua-icon.rc");

    let icon_path = icon_path
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to resolve Windows icon at {}: {err}", icon_path.display()));

    let icon_path = icon_path.to_string_lossy().replace('\\', "\\\\");
    fs::write(&rc_path, format!("1 ICON \"{icon_path}\"\n"))
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", rc_path.display()));

    embed_resource::compile(rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows icon resources");
}
