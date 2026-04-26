use std::fmt::Write as _;

const HELP_MIN_VALUE_WIDTH: usize = 25;
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
    // Keep this order stable across help, README, and completions:
    // required enablers first, then optional flags alphabetically inside each
    // namespace. Put all `--push.*` options before `--metrics.*` options.
    write_rows(
        &mut help,
        &[
            HelpRow {
                value: "--push.url URL",
                description: "push interval metrics to a Pushgateway URL",
                continuation: &["bare host:port values default to http://"],
            },
            HelpRow {
                value: "--push.delete-on-exit",
                description: "delete this Pushgateway grouping key after the run exits",
                continuation: &[],
            },
            HelpRow {
                value: "--push.interval DURATION",
                description: "aggregate interval samples before pushing window metrics",
                continuation: &[],
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
                value: "--push.retries N",
                description: "retry failed Pushgateway requests N times (default: 0)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.timeout DURATION",
                description: "per-request timeout: 500ms, 5s, 1m, or seconds (default: 5s)",
                continuation: &[],
            },
            HelpRow {
                value: "--push.user-agent VALUE",
                description: "HTTP User-Agent for Pushgateway requests",
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
            HelpRow {
                value: "--metrics.label KEY=VALUE",
                description: "add a Prometheus file sample label; repeatable",
                continuation: &["requires --metrics.format prometheus"],
            },
            HelpRow {
                value: "--metrics.prefix P",
                description: "Prometheus metric name prefix (default: iperf3)",
                continuation: &[],
            },
        ],
    );
    help.push('\n');
    help.push_str("iperf3-rs environment:\n");
    // Mirror the option order above so users can map each environment default
    // back to the CLI flag without scanning two differently sorted lists.
    write_rows(
        &mut help,
        &[
            HelpRow {
                value: "IPERF3_PUSH_URL=URL",
                description: "default value for --push.url",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_DELETE_ON_EXIT=BOOL",
                description: "default value for --push.delete-on-exit",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_INTERVAL=DURATION",
                description: "default value for --push.interval",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_JOB=JOB",
                description: "default value for --push.job",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_LABELS=KEY=VALUE,...",
                description: "default labels added before --push.label values",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_RETRIES=N",
                description: "default value for --push.retries",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_TIMEOUT=DURATION",
                description: "default value for --push.timeout",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_PUSH_USER_AGENT=VALUE",
                description: "default value for --push.user-agent",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_METRICS_FILE=PATH",
                description: "default value for --metrics.file",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_METRICS_FORMAT=FORMAT",
                description: "default value for --metrics.format",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_METRICS_LABELS=KEY=VALUE,...",
                description: "default labels for Prometheus file output",
                continuation: &[],
            },
            HelpRow {
                value: "IPERF3_METRICS_PREFIX=P",
                description: "default value for --metrics.prefix",
                continuation: &[],
            },
        ],
    );
    help
}

pub fn render_full_help(upstream_help: &str) -> String {
    let wrapper_help = render_wrapper_help();
    // Insert wrapper-only options before upstream's first semantic section. If
    // the upstream help layout changes, fall back to appending instead of
    // dropping the wrapper help entirely.
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
    // Options fit the historical iperf-style width, but environment variables
    // are longer. Compute per section so option help stays compact while env
    // help remains aligned.
    let value_width = rows
        .iter()
        .map(|row| row.value.len())
        .max()
        .unwrap_or(0)
        .max(HELP_MIN_VALUE_WIDTH);

    for row in rows {
        writeln!(help, "  {:<value_width$} {}", row.value, row.description).unwrap();
        for line in row.continuation {
            writeln!(help, "  {:<value_width$} {}", "", line).unwrap();
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
        assert_substrings_in_order(
            &help,
            &[
                "--push.url URL",
                "--push.delete-on-exit",
                "--push.interval DURATION",
                "--push.job JOB",
                "--push.label KEY=VALUE",
                "--push.retries N",
                "--push.timeout DURATION",
                "--push.user-agent VALUE",
                "--metrics.file PATH",
                "--metrics.format FORMAT",
                "--metrics.label KEY=VALUE",
                "--metrics.prefix P",
            ],
        );
        assert_substrings_in_order(
            &help,
            &[
                "IPERF3_PUSH_URL=URL",
                "IPERF3_PUSH_DELETE_ON_EXIT=BOOL",
                "IPERF3_PUSH_INTERVAL=DURATION",
                "IPERF3_PUSH_JOB=JOB",
                "IPERF3_PUSH_LABELS=KEY=VALUE,...",
                "IPERF3_PUSH_RETRIES=N",
                "IPERF3_PUSH_TIMEOUT=DURATION",
                "IPERF3_PUSH_USER_AGENT=VALUE",
                "IPERF3_METRICS_FILE=PATH",
                "IPERF3_METRICS_FORMAT=FORMAT",
                "IPERF3_METRICS_LABELS=KEY=VALUE,...",
                "IPERF3_METRICS_PREFIX=P",
            ],
        );
    }

    fn assert_substrings_in_order(haystack: &str, needles: &[&str]) {
        let mut offset = 0;
        for needle in needles {
            let Some(index) = haystack[offset..].find(needle) else {
                panic!("missing `{needle}` after byte {offset}");
            };
            offset += index + needle.len();
        }
    }

    #[test]
    fn wrapper_help_aligns_descriptions_without_tabs() {
        let help = render_wrapper_help();

        assert!(!help.contains('\t'));

        let rows = help
            .lines()
            .filter(|line| line.starts_with("  --push") || line.starts_with("  --metrics"))
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
