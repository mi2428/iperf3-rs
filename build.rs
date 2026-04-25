use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIGURE_ARGS_ENV: &str = "IPERF3_RS_CONFIGURE_ARGS";

fn main() {
    // Build libiperf from the esnet/iperf3 submodule during Cargo's build
    // script instead of expecting a system package. This keeps the Rust crate
    // pinned to the vendored upstream revision and makes the FFI surface match
    // the headers under `iperf3/src`.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();
    let profile = env::var("PROFILE").unwrap();

    let iperf_dir = manifest_dir.join("iperf3");
    let iperf_src = iperf_dir.join("src");
    let build_dir = out_dir.join("libiperf-build");
    let build_src = build_dir.join("src");
    let libiperf = build_src.join(".libs").join("libiperf.a");
    let makefile = build_src.join("Makefile");

    // Keep Cargo's rebuild triggers focused on the files that define the FFI
    // contract and on the configure options that affect the native build.
    // Autotools itself handles the full C dependency graph inside `make`.
    println!("cargo:rerun-if-changed=native/iperf3rs_shim.c");
    println!("cargo:rerun-if-changed=native/iperf3rs_shim.h");
    println!("cargo:rerun-if-changed=iperf3/configure");
    println!("cargo:rerun-if-changed=iperf3/src/iperf_api.h");
    println!("cargo:rerun-if-changed=iperf3/src/iperf.h");
    println!("cargo:rerun-if-env-changed={CONFIGURE_ARGS_ENV}");
    emit_build_metadata(&manifest_dir, &host, &target, &profile);

    if !iperf_src.join("iperf.h").exists() {
        panic!("iperf3 source is missing. Run: git submodule update --init --recursive");
    }
    // libiperf's Autotools build supports out-of-tree builds, but an earlier
    // in-source `./configure` leaves generated files in the submodule. Clean
    // those artifacts first so this build script owns the configured state.
    clean_in_source_config_if_needed(&iperf_dir);

    // Build artifacts live under OUT_DIR because Cargo may build this crate
    // for multiple targets or profiles in the same checkout. The stamp records
    // the inputs that change configure output, so we can reuse libiperf across
    // incremental Rust rebuilds without accidentally mixing host/target builds.
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

    // Compile a tiny C shim with Cargo's `cc` integration. The shim keeps Rust
    // from reaching directly into libiperf internals where the C API needs
    // callbacks or macro-shaped access, while still linking against upstream
    // libiperf without patching the submodule.
    let mut shim = cc::Build::new();
    shim.file("native/iperf3rs_shim.c")
        .include(&build_src)
        .include(&iperf_src)
        .warnings(false);
    for include_dir in configured_include_dirs(&makefile) {
        shim.include(include_dir);
    }
    shim.compile("iperf3rs_shim");

    // Link the static libiperf archive built above, then mirror any libraries
    // discovered by `configure` such as libm or optional feature libraries.
    // Reading the generated Makefile keeps this script aligned with upstream
    // configure checks instead of hard-coding platform-specific linker flags.
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

    // Configure in OUT_DIR with static output only. The final Rust binary links
    // libiperf directly, which is what allows the release Docker image to be a
    // scratch image containing only the iperf3-rs executable plus minimal
    // runtime filesystem.
    let mut configure = Command::new(iperf_dir.join("configure"));
    configure
        .current_dir(build_dir)
        .arg("--enable-static")
        .arg("--disable-shared");

    // Release and integration builds can pass upstream configure switches such
    // as `--without-openssl` without teaching this script every libiperf option.
    for arg in extra_args.split_whitespace() {
        configure.arg(arg);
    }

    // When Cargo is cross-compiling, Autotools needs its own host triple and C
    // compiler. Reusing the `cc` crate here makes the native libiperf build use
    // the same target-aware compiler selection as the shim build.
    if host != target {
        configure.arg(format!("--host={}", configure_host(target)));
        add_target_compiler_env(&mut configure, target);
    }

    run(configure, "configure iperf3");

    let mut make = Command::new("make");
    // Build only libiperf, not the upstream iperf3 CLI, because the Rust binary
    // is the frontend and only needs the library archive for FFI.
    make.current_dir(build_dir.join("src")).arg("libiperf.la");
    if let Ok(jobs) = env::var("CARGO_BUILD_JOBS") {
        make.arg(format!("-j{jobs}"));
    }
    run(make, "build libiperf");
}

fn emit_build_metadata(manifest_dir: &Path, host: &str, target: &str, profile: &str) {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=IPERF3_RS_BUILD_DATE");
    println!("cargo:rerun-if-env-changed=IPERF3_RS_GIT_DESCRIBE");
    println!("cargo:rerun-if-env-changed=IPERF3_RS_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=IPERF3_RS_GIT_COMMIT_DATE");
    emit_git_rerun_instructions(manifest_dir);

    let git_describe = read_nonempty_env("IPERF3_RS_GIT_DESCRIBE")
        .or_else(|| {
            git_output(
                manifest_dir,
                ["describe", "--tags", "--always", "--dirty=-dirty"],
            )
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let git_commit = read_nonempty_env("IPERF3_RS_GIT_COMMIT")
        .or_else(|| git_output(manifest_dir, ["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    let git_commit_date = read_nonempty_env("IPERF3_RS_GIT_COMMIT_DATE")
        .or_else(|| git_output(manifest_dir, ["show", "-s", "--format=%cI", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    let build_date = read_nonempty_env("IPERF3_RS_BUILD_DATE").unwrap_or_else(build_date);

    println!("cargo:rustc-env=IPERF3_RS_GIT_DESCRIBE={git_describe}");
    println!("cargo:rustc-env=IPERF3_RS_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=IPERF3_RS_GIT_COMMIT_DATE={git_commit_date}");
    println!("cargo:rustc-env=IPERF3_RS_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=IPERF3_RS_BUILD_HOST={host}");
    println!("cargo:rustc-env=IPERF3_RS_BUILD_TARGET={target}");
    println!("cargo:rustc-env=IPERF3_RS_BUILD_PROFILE={profile}");
}

fn emit_git_rerun_instructions(manifest_dir: &Path) {
    let git = manifest_dir.join(".git");
    if git.is_file() {
        println!("cargo:rerun-if-changed={}", git.display());
        let Ok(contents) = fs::read_to_string(&git) else {
            return;
        };
        let Some(git_dir) = contents.trim().strip_prefix("gitdir: ") else {
            return;
        };
        let git_dir = absolutize_git_path(manifest_dir, git_dir);
        emit_git_dir_rerun_instructions(&git_dir);
        return;
    }
    if !git.is_dir() {
        return;
    }

    emit_git_dir_rerun_instructions(&git);
}

fn emit_git_dir_rerun_instructions(git_dir: &Path) {
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) else {
        return;
    };
    if let Some(ref_name) = head.trim().strip_prefix("ref: ") {
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(ref_name).display()
        );
    }
}

fn absolutize_git_path(manifest_dir: &Path, git_dir: &str) -> PathBuf {
    let path = PathBuf::from(git_dir);
    if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    }
}

fn git_output<const N: usize>(manifest_dir: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn read_nonempty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn build_date() -> String {
    let seconds = read_nonempty_env("SOURCE_DATE_EPOCH")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default()
        });
    unix_seconds_to_utc_iso8601(seconds)
}

fn unix_seconds_to_utc_iso8601(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_parameter = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_parameter + 2) / 5 + 1;
    let month = month_parameter + if month_parameter < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    (year, month as u32, day as u32)
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
