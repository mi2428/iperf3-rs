use std::fmt::Write;

#[derive(Debug, Clone, Copy)]
pub struct VersionInfo<'a> {
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub libiperf_version: &'a str,
    pub git_describe: &'a str,
    pub git_commit: &'a str,
    pub git_commit_date: &'a str,
    pub build_date: &'a str,
    pub build_host: &'a str,
    pub build_target: &'a str,
    pub build_profile: &'a str,
}

pub fn current(libiperf_version: &str) -> VersionInfo<'_> {
    VersionInfo {
        package_name: env!("CARGO_PKG_NAME"),
        package_version: env!("CARGO_PKG_VERSION"),
        libiperf_version,
        git_describe: env!("IPERF3_RS_GIT_DESCRIBE"),
        git_commit: env!("IPERF3_RS_GIT_COMMIT"),
        git_commit_date: env!("IPERF3_RS_GIT_COMMIT_DATE"),
        build_date: env!("IPERF3_RS_BUILD_DATE"),
        build_host: env!("IPERF3_RS_BUILD_HOST"),
        build_target: env!("IPERF3_RS_BUILD_TARGET"),
        build_profile: env!("IPERF3_RS_BUILD_PROFILE"),
    }
}

pub fn render(info: &VersionInfo<'_>) -> String {
    let mut out = String::new();
    writeln!(out, "{} {}", info.package_name, info.package_version).unwrap();
    writeln!(out, "esnet/iperf3 libiperf {}", info.libiperf_version).unwrap();
    writeln!(out, "git describe: {}", info.git_describe).unwrap();
    writeln!(out, "git commit: {}", info.git_commit).unwrap();
    writeln!(out, "git commit date: {}", info.git_commit_date).unwrap();
    writeln!(out, "build date: {}", info.build_date).unwrap();
    writeln!(out, "build host: {}", info.build_host).unwrap();
    writeln!(out, "build target: {}", info.build_target).unwrap();
    writeln!(out, "build profile: {}", info.build_profile).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_wrapper_and_upstream_versions() {
        let rendered = render(&VersionInfo {
            package_name: "iperf3-rs",
            package_version: "0.1.0",
            libiperf_version: "3.20",
            git_describe: "v0.1.0-1-gabc123",
            git_commit: "abc123",
            git_commit_date: "2026-04-25T00:00:00+09:00",
            build_date: "2026-04-25T01:00:00Z",
            build_host: "aarch64-apple-darwin",
            build_target: "x86_64-unknown-linux-gnu",
            build_profile: "release",
        });

        assert!(rendered.contains("iperf3-rs 0.1.0\n"));
        assert!(rendered.contains("esnet/iperf3 libiperf 3.20\n"));
        assert!(rendered.contains("git describe: v0.1.0-1-gabc123\n"));
        assert!(rendered.contains("build target: x86_64-unknown-linux-gnu\n"));
    }
}
