use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc_exe() -> PathBuf {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return PathBuf::from("rc.exe");
    }

    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits_bin) {
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        for dir in versions.into_iter().rev() {
            let cand = dir.join("x64").join("rc.exe");
            if cand.is_file() {
                return cand;
            }
        }
    }

    panic!("Windows SDK rc.exe not found in PATH or C:\\Program Files (x86)\\Windows Kits\\10\\bin");
}

fn main() {
    println!("cargo:rerun-if-changed=assets/selabar.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        let out_res = out_dir.join("selabar.res");
        let rc = find_rc_exe();

        let status = Command::new(&rc)
            .args([
                "/nologo",
                "/fo",
                out_res.to_str().expect("Invalid OUT_DIR path"),
                "assets/selabar.rc",
            ])
            .status()
            .unwrap_or_else(|e| panic!("Failed to launch {:?}: {}", rc, e));

        if !status.success() {
            panic!("rc.exe failed with exit status: {}", status);
        }

        println!("cargo:rustc-link-arg-bin=SelaBar={}", out_res.display());
        println!("cargo:rustc-link-arg-bin=SelaBar-Setup={}", out_res.display());
    }
}
