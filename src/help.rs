use std::fmt::Write as _;

const HELP_VALUE_WIDTH: usize = 25;

struct HelpRow<'a> {
    value: &'a str,
    description: &'a str,
    continuation: &'a [&'a str],
}

pub fn render_wrapper_help() -> String {
    // Upstream owns the iperf3 option list. Keep this section limited to the
    // options consumed by the Rust frontend before argv reaches libiperf.
    let mut help = String::new();
    help.push('\n');
    help.push_str("iperf3-rs options:\n");
    write_rows(
        &mut help,
        &[
            HelpRow {
                value: "--push.url URL",
                description: "push interval metrics to a Pushgateway URL",
                continuation: &["bare host:port values default to http://"],
            },
            HelpRow {
                value: "--push.job JOB",
                description: "Pushgateway job name (default: iperf3)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.label KEY=VALUE",
                description: "add a Pushgateway grouping label; repeatable",
                continuation: &[],
            },
            HelpRow {
                value: "--push.timeout DURATION",
                description: "per-request timeout: 500ms, 5s, 1m, or seconds (default: 5s)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.retries N",
                description: "retry failed Pushgateway requests N times (default: 0)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.user-agent VALUE",
                description: "HTTP User-Agent for Pushgateway requests",
                continuation: &[],
            },
            HelpRow {
                value: "--push.metric-prefix P",
                description: "Prometheus metric name prefix (default: iperf3)",
                continuation: &[],
            },
        ],
    );
    help.push('\n');
    help.push_str("iperf3-rs environment:\n");
    write_rows(
        &mut help,
        &[
            HelpRow {
                value: "PUSH_URL=URL",
                description: "default value for --push.url",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_JOB=JOB",
                description: "default value for --push.job",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_LABELS=KEY=VALUE,...",
                description: "default labels added before --push.label values",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_TIMEOUT=DURATION",
                description: "default value for --push.timeout",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_RETRIES=N",
                description: "default value for --push.retries",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_USER_AGENT=VALUE",
                description: "default value for --push.user-agent",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_METRIC_PREFIX=P",
                description: "default value for --push.metric-prefix",
                continuation: &[],
            },
        ],
    );
    help
}

fn write_rows(help: &mut String, rows: &[HelpRow<'_>]) {
    for row in rows {
        writeln!(
            help,
            "  {:<HELP_VALUE_WIDTH$} {}",
            row.value, row.description
        )
        .unwrap();
        for line in row.continuation {
            writeln!(help, "  {:<HELP_VALUE_WIDTH$} {}", "", line).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_help_documents_pushgateway_options() {
        let help = render_wrapper_help();

        assert!(help.contains("iperf3-rs options:"));
        assert!(help.contains("--push.url URL"));
        assert!(help.contains("--push.job JOB"));
        assert!(help.contains("--push.label KEY=VALUE"));
        assert!(help.contains("--push.timeout DURATION"));
        assert!(help.contains("--push.retries N"));
        assert!(help.contains("--push.user-agent VALUE"));
        assert!(help.contains("--push.metric-prefix P"));
        assert!(help.contains("PUSH_LABELS=KEY=VALUE,..."));
        assert!(help.contains("PUSH_METRIC_PREFIX=P"));
    }

    #[test]
    fn wrapper_help_aligns_descriptions_without_tabs() {
        let help = render_wrapper_help();

        assert!(!help.contains('\t'));

        let rows = help
            .lines()
            .filter(|line| line.starts_with("  --push") || line.starts_with("  PUSH_"))
            .collect::<Vec<_>>();
        assert!(
            rows.iter()
                .all(|line| line.as_bytes()[27].is_ascii_whitespace()
                    && !line.as_bytes()[28].is_ascii_whitespace())
        );
    }
}
