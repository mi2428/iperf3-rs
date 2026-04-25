use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIGURE_ARGS_ENV: &str = "IPERF3_RS_CONFIGURE_ARGS";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();

    let iperf_dir = manifest_dir.join("iperf3");
    let iperf_src = iperf_dir.join("src");
    let build_dir = out_dir.join("libiperf-build");
    let build_src = build_dir.join("src");
    let libiperf = build_src.join(".libs").join("libiperf.a");
    let makefile = build_src.join("Makefile");

    println!("cargo:rerun-if-changed=native/iperf3rs_shim.c");
    println!("cargo:rerun-if-changed=native/iperf3rs_shim.h");
    println!("cargo:rerun-if-changed=iperf3/configure");
    println!("cargo:rerun-if-changed=iperf3/src/iperf_api.h");
    println!("cargo:rerun-if-changed=iperf3/src/iperf.h");
    println!("cargo:rerun-if-env-changed={CONFIGURE_ARGS_ENV}");

    if !iperf_src.join("iperf.h").exists() {
        panic!("iperf3 source is missing. Run: git submodule update --init --recursive");
    }
    clean_in_source_config_if_needed(&iperf_dir);

    let configure_args = env::var(CONFIGURE_ARGS_ENV).unwrap_or_default();
    let stamp = format!("target={target}\nhost={host}\nconfigure_args={configure_args}\n");
    if !libiperf.exists() || read_stamp(&build_dir).as_deref() != Some(stamp.as_str()) {
        if build_dir.exists() {
            fs::remove_dir_all(&build_dir).unwrap_or_else(|err| {
                panic!("failed to remove stale libiperf build directory: {err}")
            });
        }
        configure_and_build_iperf(&iperf_dir, &build_dir, &host, &target, &configure_args);
        fs::write(build_dir.join(".iperf3-rs-build-stamp"), stamp)
            .unwrap_or_else(|err| panic!("failed to write libiperf build stamp: {err}"));
    }

    let mut shim = cc::Build::new();
    shim.file("native/iperf3rs_shim.c")
        .include(&build_src)
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

fn configure_and_build_iperf(
    iperf_dir: &Path,
    build_dir: &Path,
    host: &str,
    target: &str,
    extra_args: &str,
) {
    fs::create_dir_all(build_dir)
        .unwrap_or_else(|err| panic!("failed to create libiperf build directory: {err}"));

    let mut configure = Command::new(iperf_dir.join("configure"));
    configure
        .current_dir(build_dir)
        .arg("--enable-static")
        .arg("--disable-shared");

    for arg in extra_args.split_whitespace() {
        configure.arg(arg);
    }

    if host != target {
        configure.arg(format!("--host={}", configure_host(target)));
        add_target_compiler_env(&mut configure, target);
    }

    run(configure, "configure iperf3");

    let mut make = Command::new("make");
    make.current_dir(build_dir.join("src")).arg("libiperf.la");
    if let Ok(jobs) = env::var("CARGO_BUILD_JOBS") {
        make.arg(format!("-j{jobs}"));
    }
    run(make, "build libiperf");
}

fn clean_in_source_config_if_needed(iperf_dir: &Path) {
    if !iperf_dir.join("config.status").exists() {
        return;
    }

    let mut make = Command::new("make");
    make.current_dir(iperf_dir).arg("distclean");
    run(make, "clean in-source iperf3 configure artifacts");
}

fn add_target_compiler_env(command: &mut Command, target: &str) {
    let compiler = cc::Build::new().target(target).get_compiler();
    command.env("CC", compiler.path());

    let flags = join_os_args(compiler.args());
    if !flags.is_empty() {
        command.env("CFLAGS", &flags);
        command.env("LDFLAGS", &flags);
    }
}

fn configure_host(target: &str) -> &str {
    match target {
        "x86_64-apple-darwin" => "x86_64-apple-darwin",
        "aarch64-apple-darwin" => "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu" => "x86_64-pc-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-unknown-linux-gnu",
        other => other,
    }
}

fn join_os_args(args: &[impl AsRef<OsStr>]) -> String {
    args.iter()
        .map(|arg| arg.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
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
                } else if let Some(lib) = token.strip_prefix("-l")
                    && !lib.is_empty()
                {
                    println!("cargo:rustc-link-lib={lib}");
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
                if let Some(path) = token.strip_prefix("-I")
                    && !path.is_empty()
                {
                    dirs.push(path.to_owned());
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

fn read_stamp(build_dir: &Path) -> Option<String> {
    fs::read_to_string(build_dir.join(".iperf3-rs-build-stamp")).ok()
}
