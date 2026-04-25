use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let iperf_dir = manifest_dir.join("iperf3");
    let iperf_src = iperf_dir.join("src");
    let libiperf = iperf_src.join(".libs").join("libiperf.a");

    println!("cargo:rerun-if-changed=native/iperf3rs_shim.c");
    println!("cargo:rerun-if-changed=native/iperf3rs_shim.h");
    println!("cargo:rerun-if-changed=iperf3/src/iperf_api.h");
    println!("cargo:rerun-if-changed=iperf3/src/iperf.h");

    if !iperf_src.join("iperf.h").exists() {
        panic!("iperf3 source is missing. Clone esnet/iperf into ./iperf3 first.");
    }

    if !libiperf.exists() {
        configure_and_build_iperf(&iperf_dir);
    }

    let makefile = iperf_src.join("Makefile");
    let mut shim = cc::Build::new();
    shim.file("native/iperf3rs_shim.c")
        .include(&iperf_src)
        .warnings(false);
    for include_dir in configured_include_dirs(&makefile) {
        shim.include(include_dir);
    }
    shim.compile("iperf3rs_shim");

    println!(
        "cargo:rustc-link-search=native={}",
        libiperf.parent().unwrap().display()
    );
    println!("cargo:rustc-link-lib=static=iperf");
    emit_configured_link_flags(&makefile);
}

fn configure_and_build_iperf(iperf_dir: &Path) {
    let mut configure = Command::new("./configure");
    configure
        .current_dir(iperf_dir)
        .arg("--enable-static")
        .arg("--disable-shared");

    run(configure, "configure iperf3");

    let mut make = Command::new("make");
    make.current_dir(iperf_dir.join("src")).arg("libiperf.la");
    if let Ok(jobs) = env::var("CARGO_BUILD_JOBS") {
        make.arg(format!("-j{jobs}"));
    }
    run(make, "build libiperf");
}

fn run(mut command: Command, what: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("failed to {what}: {err}"));
    if !status.success() {
        panic!("{what} failed with status {status}");
    }
}

fn emit_configured_link_flags(makefile: &Path) {
    let Ok(contents) = fs::read_to_string(makefile) else {
        println!("cargo:rustc-link-lib=m");
        return;
    };

    for key in ["LDFLAGS", "LIBS"] {
        if let Some(value) = read_make_var(&contents, key) {
            for token in value.split_whitespace() {
                if let Some(path) = token.strip_prefix("-L") {
                    if !path.is_empty() {
                        println!("cargo:rustc-link-search=native={path}");
                    }
                } else if let Some(lib) = token.strip_prefix("-l") {
                    if !lib.is_empty() {
                        println!("cargo:rustc-link-lib={lib}");
                    }
                }
            }
        }
    }
}

fn configured_include_dirs(makefile: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(makefile) else {
        return Vec::new();
    };

    let mut dirs = Vec::new();
    for key in ["CPPFLAGS", "OPENSSL_INCLUDES"] {
        if let Some(value) = read_make_var(&contents, key) {
            for token in value.split_whitespace() {
                if let Some(path) = token.strip_prefix("-I") {
                    if !path.is_empty() {
                        dirs.push(path.to_owned());
                    }
                }
            }
        }
    }
    dirs
}

fn read_make_var(contents: &str, key: &str) -> Option<String> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key} = ")).map(str::to_owned))
}
