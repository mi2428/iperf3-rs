use std::fmt::Write as _;

const HELP_VALUE_WIDTH: usize = 25;
const UPSTREAM_FIRST_SECTION: &str = "Server or Client:\n";

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
                value: "--metrics.prefix P",
                description: "Prometheus metric name prefix (default: iperf3)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.metric-prefix P",
                description: "deprecated alias for --metrics.prefix",
                continuation: &[],
            },
            HelpRow {
                value: "--push.interval DURATION",
                description: "aggregate interval samples before pushing window metrics",
                continuation: &[],
            },
            HelpRow {
                value: "--push.delete-on-exit",
                description: "delete this Pushgateway grouping key after the run exits",
                continuation: &[],
            },
            HelpRow {
                value: "--metrics.file PATH",
                description: "write live interval metrics to a file",
                continuation: &["does not change iperf stdout"],
            },
            HelpRow {
                value: "--metrics.format FORMAT",
                description: "metrics file format: jsonl or prometheus (default: jsonl)",
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
                value: "METRICS_PREFIX=P",
                description: "default value for --metrics.prefix",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_METRIC_PREFIX=P",
                description: "deprecated alias for METRICS_PREFIX",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_INTERVAL=DURATION",
                description: "default value for --push.interval",
                continuation: &[],
            },
            HelpRow {
                value: "PUSH_DELETE_ON_EXIT=BOOL",
                description: "default value for --push.delete-on-exit",
                continuation: &[],
            },
            HelpRow {
                value: "METRICS_FILE=PATH",
                description: "default value for --metrics.file",
                continuation: &[],
            },
            HelpRow {
                value: "METRICS_FORMAT=FORMAT",
                description: "default value for --metrics.format",
                continuation: &[],
            },
        ],
    );
    help
}

pub fn render_full_help(upstream_help: &str) -> String {
    let wrapper_help = render_wrapper_help();
    if let Some(index) = upstream_help.find(UPSTREAM_FIRST_SECTION) {
        let (usage, upstream_sections) = upstream_help.split_at(index);
        let mut help = String::with_capacity(upstream_help.len() + wrapper_help.len() + 1);
        help.push_str(usage);
        help.push_str(wrapper_help.trim_start_matches('\n'));
        help.push('\n');
        help.push_str(upstream_sections);
        help
    } else {
        let mut help = upstream_help.to_owned();
        if !help.ends_with('\n') {
            help.push('\n');
        }
        help.push_str(&wrapper_help);
        help
    }
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
        assert!(help.contains("--metrics.prefix P"));
        assert!(help.contains("--push.metric-prefix P"));
        assert!(help.contains("--push.interval DURATION"));
        assert!(help.contains("--push.delete-on-exit"));
        assert!(help.contains("--metrics.file PATH"));
        assert!(help.contains("--metrics.format FORMAT"));
        assert!(help.contains("PUSH_LABELS=KEY=VALUE,..."));
        assert!(help.contains("METRICS_PREFIX=P"));
        assert!(help.contains("PUSH_METRIC_PREFIX=P"));
        assert!(help.contains("PUSH_INTERVAL=DURATION"));
        assert!(help.contains("PUSH_DELETE_ON_EXIT=BOOL"));
        assert!(help.contains("METRICS_FILE=PATH"));
        assert!(help.contains("METRICS_FORMAT=FORMAT"));
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

    #[test]
    fn full_help_inserts_wrapper_help_before_upstream_sections() {
        let help = render_full_help(concat!(
            "Usage: iperf3 [-s|-c host] [options]\n",
            "       iperf3 [-h|--help] [-v|--version]\n",
            "\n",
            "Server or Client:\n",
            "  -p, --port      #         server port to listen on/connect to\n",
            "\n",
            "Report bugs to:     https://github.com/esnet/iperf\n",
        ));

        let wrapper_index = help.find("iperf3-rs options:\n").unwrap();
        let server_index = help.find("Server or Client:\n").unwrap();
        let bug_report_index = help.find("Report bugs to:").unwrap();

        assert!(wrapper_index < server_index);
        assert!(server_index < bug_report_index);
        assert!(help.contains("Report bugs to:     https://github.com/esnet/iperf\n"));
    }
}
