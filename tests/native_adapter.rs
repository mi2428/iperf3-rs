#[path = "../native/source_adapter.rs"]
mod source_adapter;

#[test]
fn reviewed_upstream_adaptations_fail_closed_on_drift() {
    let api = include_str!("../iperf3/src/iperf_api.c");
    let server = include_str!("../iperf3/src/iperf_server_api.c");
    let client = include_str!("../iperf3/src/iperf_client_api.c");
    let net = include_str!("../iperf3/src/net.c");
    assert!(source_adapter::api(api).contains("return 3; /* invalid or missing option */"));
    assert!(source_adapter::server(server).contains("return 0; /* one-off idle completion */"));
    let metadata = source_adapter::option_metadata(api);
    assert!(metadata.contains("{\"timestamps\", optional_argument, NULL, OPT_TIMESTAMPS}"));
    assert!(metadata.contains("{\"extra-data\", required_argument, NULL, OPT_EXTRA_DATA}"));
    assert!(metadata.contains("static const char iperf3rs_shortopts[]"));
    assert!(source_adapter::client(client).contains("iperf3rs_cli_select"));
    assert!(source_adapter::client(client).contains(
        "#endif // __vxworks or __VXWORKS__\n        if (iperf3rs_cli_interrupted()) goto cleanup_and_fail;"
    ));
    assert!(source_adapter::net(net).contains("iperf3rs_cli_select"));
    for changed in [api.replace("exit(1);", "exit(2);"), format!("{api}\n{api}")] {
        assert!(std::panic::catch_unwind(|| source_adapter::api(&changed)).is_err());
    }
}
