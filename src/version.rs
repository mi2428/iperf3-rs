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
        git_describe: env!("IPERF3_GIT_DESCRIBE"),
        git_commit: env!("IPERF3_GIT_COMMIT"),
        git_commit_date: env!("IPERF3_GIT_COMMIT_DATE"),
        build_date: env!("IPERF3_BUILD_DATE"),
        build_host: env!("IPERF3_BUILD_HOST"),
        build_target: env!("IPERF3_BUILD_TARGET"),
        build_profile: env!("IPERF3_BUILD_PROFILE"),
    }
}

pub fn render(info: &VersionInfo<'_>) -> String {
    // Keep --version script-friendly: one line, with the same broad shape as
    // common runtimes (`name version (revision, date) [dependency] on target`).
    format!(
        "{} {} (git {}; commit {}; commit date {}; built {}; {}) [libiperf {}] on {} (host {})\n",
        info.package_name,
        info.package_version,
        info.git_describe,
        info.git_commit,
        info.git_commit_date,
        info.build_date,
        info.build_profile,
        info.libiperf_version,
        info.build_target,
        info.build_host,
    )
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

        assert_eq!(
            rendered,
            concat!(
                "iperf3-rs 0.1.0 ",
                "(git v0.1.0-1-gabc123; commit abc123; ",
                "commit date 2026-04-25T00:00:00+09:00; ",
                "built 2026-04-25T01:00:00Z; release) ",
                "[libiperf 3.20] on x86_64-unknown-linux-gnu ",
                "(host aarch64-apple-darwin)\n"
            )
        );
    }
}
