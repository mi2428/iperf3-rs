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
        "\n",
        "iperf3-rs environment:\n",
        "  PUSH_URL=URL             default value for --push.url\n",
        "  PUSH_JOB=JOB             default value for --push.job\n",
        "  PUSH_LABELS=KEY=VALUE,...\n",
        "                           default labels added before --push.label values\n",
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
        assert!(help.contains("PUSH_LABELS=KEY=VALUE,..."));
    }
}
