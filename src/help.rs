pub fn render_wrapper_help() -> &'static str {
    // Upstream owns the iperf3 option list. Keep this section limited to the
    // options consumed by the Rust frontend before argv reaches libiperf.
    concat!(
        "\n",
        "iperf3-rs options:\n",
        "  --push.url URL           push interval metrics to a Pushgateway URL\n",
        "                           bare host:port values default to http://\n",
        "  --push.job JOB           Pushgateway job name (default: iperf3)\n",
        "  --push.label KEY=VALUE   add a Pushgateway grouping label; repeatable\n",
        "  --push.timeout DURATION  per-request timeout: 500ms, 5s, 1m, or seconds (default: 5s)\n",
        "  --push.retries N         retry failed Pushgateway requests N times (default: 0)\n",
        "  --push.user-agent VALUE  HTTP User-Agent for Pushgateway requests\n",
        "  --push.metric-prefix P   Prometheus metric name prefix (default: iperf3)\n",
        "\n",
        "iperf3-rs environment:\n",
        "  PUSH_URL=URL             default value for --push.url\n",
        "  PUSH_JOB=JOB             default value for --push.job\n",
        "  PUSH_LABELS=KEY=VALUE,...\n",
        "                           default labels added before --push.label values\n",
        "  PUSH_TIMEOUT=DURATION    default value for --push.timeout\n",
        "  PUSH_RETRIES=N           default value for --push.retries\n",
        "  PUSH_USER_AGENT=VALUE    default value for --push.user-agent\n",
        "  PUSH_METRIC_PREFIX=P     default value for --push.metric-prefix\n",
    )
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
}
