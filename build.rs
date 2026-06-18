//! Embeds the application icon and manifest (DPI awareness, Win10/11 compat,
//! v6 common controls) into the executable.
//!
//! We invoke `windres` directly rather than via a helper crate because the
//! project path can contain spaces (e.g. `...\mini projects\...`). `windres`
//! forwards include paths to the C preprocessor, which splits unquoted spaces;
//! running from the `assets` dir with *relative* references avoids passing any
//! space-containing path to the preprocessor.
//!
//! If `windres` is unavailable the build still succeeds — the tray icon is
//! created at runtime from an embedded PNG regardless of the resource embed.

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/claude-clip.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let res_o = Path::new(&out_dir).join("app_res.o");

    let bfd = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "pe-i386",
        _ => "pe-x86-64",
    };
    let windres = env::var("WINDRES").unwrap_or_else(|_| "windres".to_string());

    let status = Command::new(&windres)
        .current_dir("assets")
        .arg("--target")
        .arg(bfd)
        .arg("app.rc")
        .arg(&res_o)
        .status();

    match status {
        Ok(s) if s.success() => {
            // Link the compiled resource object into the binary only.
            println!("cargo:rustc-link-arg-bins={}", res_o.display());
        }
        Ok(s) => println!(
            "cargo:warning=windres exited with {s}; building without embedded icon/manifest"
        ),
        Err(e) => println!(
            "cargo:warning=could not run windres ({e}); building without embedded icon/manifest"
        ),
    }
}
