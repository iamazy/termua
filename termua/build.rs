use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../locales");
    println!("cargo:rerun-if-changed=../assets/logo/termua.ico");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR"));
    let git_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    fs::write(out_dir.join("git-commit.txt"), git_hash.as_bytes())
        .expect("failed to write git-commit.txt");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let icon_path = manifest_dir.join("../assets/logo/termua.ico");
    let rc_path = out_dir.join("termua-icon.rc");

    let icon_path = icon_path.canonicalize().unwrap_or_else(|err| {
        panic!(
            "failed to resolve Windows icon at {}: {err}",
            icon_path.display()
        )
    });

    let icon_path = icon_path.to_string_lossy().replace('\\', "\\\\");
    fs::write(&rc_path, format!("1 ICON \"{icon_path}\"\n"))
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", rc_path.display()));

    embed_resource::compile(rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows icon resources");
}
