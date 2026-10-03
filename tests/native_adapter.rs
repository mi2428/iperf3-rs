#[path = "../native/source_adapter.rs"]
mod source_adapter;

#[test]
fn reviewed_upstream_adaptations_fail_closed_on_drift() {
    let api = include_str!("../iperf3/src/iperf_api.c");
    let server = include_str!("../iperf3/src/iperf_server_api.c");
    assert!(source_adapter::api(api).contains("return 3; /* invalid or missing option */"));
    assert!(source_adapter::server(server).contains("return 0; /* one-off idle completion */"));
    for changed in [api.replace("exit(1);", "exit(2);"), format!("{api}\n{api}")] {
        assert!(std::panic::catch_unwind(|| source_adapter::api(&changed)).is_err());
    }
}
