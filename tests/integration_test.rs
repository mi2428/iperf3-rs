mod support;

use support::*;

// This integration test exercises the Docker Compose test topology end to end.
//
// The topology contains:
// - `server-rs`: an iperf3-rs server with Pushgateway defaults in its
//   environment, used to verify that the Rust frontend can accept upstream
//   iperf3 traffic and export server-role interval metrics.
// - `reference`: an upstream esnet/iperf3 server, used to verify that the
//   iperf3-rs client remains wire-compatible with the reference implementation.
// - `pushgateway`: a Prometheus Pushgateway instance, used to verify that
//   iperf3-rs can publish interval metrics during client and server runs.
// - `client-rs`: the test runner image, which contains both iperf3-rs and the
//   upstream iperf3 binary and has Pushgateway defaults in its environment so
//   the same container can drive all scenarios.
//
// The test succeeds only when all of the following are true:
// - the Compose image builds and the server services start;
// - Pushgateway reports readiness;
// - an upstream iperf3 client can complete a JSON test against iperf3-rs;
// - the metrics-enabled iperf3-rs server publishes non-zero
//   `iperf3_bytes` and `iperf3_bandwidth` samples with the expected
//   integration labels;
// - that same metrics-enabled server keeps the normal upstream human-readable
//   stdout shape instead of going silent;
// - an iperf3-rs client can complete a JSON test against upstream iperf3;
// - an iperf3-rs client with metrics enabled still preserves `-J` as a normal
//   complete JSON document rather than turning it into JSON stream output;
// - an iperf3-rs client run from the metrics-enabled `client-rs` service
//   publishes non-zero `iperf3_bytes` and `iperf3_bandwidth` samples with the
//   expected integration labels;
// - TCP runs expose sender-side TCP_INFO metrics while UDP-only metrics stay at
//   zero for TCP scenarios;
// - a metrics-enabled iperf3-rs client without `-J` keeps normal
//   human-readable stdout;
// - client metrics appear in Pushgateway while a longer run is still active;
// - CLI file metrics can be written in Prometheus format without replacing
//   normal iperf stdout, while Pushgateway export still receives the same run;
// - `--push.delete-on-exit` publishes live metrics during a run and removes the
//   retained Pushgateway group after the run exits;
// - `--push.interval` publishes aggregated `iperf3_window_*` metrics instead
//   of the immediate interval metric families for that grouping key;
// - the long-running iperf3-rs server continues pushing metrics after a later
//   client connection;
// - UDP, reverse TCP, and bidirectional TCP runs all complete and publish
//   protocol-specific metrics through the same environment-based Pushgateway path.
#[test]
#[ignore = "requires Docker"]
fn compose_interop_and_pushgateway_metrics() {
    let project = ComposeProject::new();

    // Build the shared test image and start the long-running services. The
    // `ComposeProject` uses a unique project name so concurrent or stale test
    // runs do not reuse containers from another invocation.
    if !truthy_env("SKIP_INTEGRATION_IMAGE_BUILD") {
        project.run_compose(&["build", "client-rs"]);
    }
    project.run_compose(&["up", "-d", "server-rs", "reference", "pushgateway"]);

    // Wait until Pushgateway is ready to accept writes. This prevents the
    // metrics assertion from racing the container startup path.
    wait_for("pushgateway readiness", || {
        project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/-/ready")])
    });

    // Interop check 1: the upstream iperf3 client must be able to talk to the
    // iperf3-rs server and return a complete JSON summary containing traffic.
    let upstream_to_iperf3rs = retry_json_client("upstream client to iperf3-rs server", || {
        project.client_output(&["iperf3", "-c", "server-rs", "-t", "3", "-i", "1", "-J"])
    });
    assert_iperf_summary_has_traffic(&upstream_to_iperf3rs);

    // Server metrics check: because `server-rs` itself gets Pushgateway
    // configuration from its service environment, the upstream client traffic
    // above should leave a server-side metric group in Pushgateway. This keeps
    // the topology close to the real deployment shape instead of adding a
    // dedicated one-off metrics server.
    wait_for_pushgateway_metrics(
        &project,
        SERVER_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert_metric_absent(&project, "iperf3_udp_packets", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_lost_packets", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_jitter_seconds", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_tcp_rtt_seconds", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_out_of_order_packets", SERVER_SCENARIO);
    assert_metric_value_eq(&project, "iperf3_omitted", SERVER_SCENARIO, 0.0);
    wait_for_service_log_contains(&project, "server-rs", "Server listening on");
    wait_for_service_log_contains(&project, "server-rs", "[ ID]");
    let first_server_push =
        wait_for_metric_value_gt(&project, "push_time_seconds", SERVER_SCENARIO, 0.0);

    // Interop check 2: the iperf3-rs client must be able to talk to the
    // upstream iperf3 server and return a complete JSON summary containing
    // traffic. This covers the opposite client/server direction. The scenario
    // override keeps metrics from this reference-server run separate from the
    // iperf3-rs-to-iperf3-rs client metrics asserted below.
    let iperf3rs_to_upstream_output =
        retry_json_client_output("iperf3-rs client to upstream server", || {
            project.client_output(&[
                "iperf3-rs",
                "--push.label",
                "scenario=tcp-reference",
                "-c",
                "reference",
                "-t",
                "1",
                "-J",
            ])
        });
    assert_stdout_is_json_document(&iperf3rs_to_upstream_output);
    let iperf3rs_to_upstream =
        parse_iperf_summary(&iperf3rs_to_upstream_output).expect("iperf JSON should parse");
    assert_iperf_summary_has_traffic(&iperf3rs_to_upstream);

    // Metrics check: run iperf3-rs against iperf3-rs. Pushgateway
    // configuration comes from the `client-rs` service environment, so this is
    // deliberately just a normal iperf client command.
    let iperf3rs_to_iperf3rs_output = project.client_output(&[
        "iperf3-rs",
        "--push.label",
        "scenario=tcp",
        "-c",
        "server-rs",
        "-t",
        "3",
        "-i",
        "1",
    ]);
    assert_success(
        &["iperf3-rs metrics-enabled human stdout"],
        &iperf3rs_to_iperf3rs_output,
    );
    assert_human_iperf_output(&iperf3rs_to_iperf3rs_output);

    // Scrape Pushgateway and require non-zero traffic plus TCP_INFO samples.
    // The metric names prove that the pusher emitted the expected metric
    // families; the label filters prove they came from this integration
    // scenario rather than from another stale Pushgateway group.
    wait_for_pushgateway_metrics(
        &project,
        CLIENT_SCENARIO,
        &[
            "iperf3_bytes",
            "iperf3_bandwidth",
            "iperf3_tcp_rtt_seconds",
            "iperf3_tcp_rttvar_seconds",
            "iperf3_tcp_snd_cwnd_bytes",
            "iperf3_tcp_snd_wnd_bytes",
            "iperf3_tcp_pmtu_bytes",
        ],
    );
    assert_metric_absent(&project, "iperf3_udp_packets", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_lost_packets", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_jitter_seconds", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_out_of_order_packets", CLIENT_SCENARIO);
    assert_metric_value_eq(&project, "iperf3_omitted", CLIENT_SCENARIO, 0.0);

    // The same long-running server should keep its callback and Pushgateway
    // configuration across client connections. Pushgateway maintains
    // `push_time_seconds` per grouping key, so requiring it to increase after
    // the second connection catches regressions where server callbacks only
    // work for the first accepted test.
    wait_for_metric_value_gt(
        &project,
        "push_time_seconds",
        SERVER_SCENARIO,
        first_server_push,
    );

    // Live push check: the client process runs long enough that metrics should
    // be visible before the iperf command exits. This directly protects the
    // interval-push behavior rather than only observing the final retained
    // Pushgateway sample after process completion.
    let live_args = [
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-live",
        "-c",
        "server-rs",
        "-t",
        "6",
        "-i",
        "1",
    ];
    let mut live_client = project.spawn_client(&live_args);
    wait_for_pushgateway_metrics(
        &project,
        LIVE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert!(
        live_client
            .try_wait()
            .expect("failed to poll live client")
            .is_none(),
        "live client exited before interval metrics were observed"
    );
    assert_child_success(&live_args, live_client);

    // File metrics check: use the same Compose service that has Pushgateway
    // defaults to prove the callback fan-out can write a Prometheus metrics file
    // and still leave normal iperf stdout untouched.
    let file_output = project.client_output(&[
        "sh",
        "-eu",
        "-c",
        r#"
metrics=/tmp/iperf3-rs-file.prom
stdout=/tmp/iperf3-rs-file.out
iperf3-rs \
  --push.label scenario=tcp-file \
  --metrics.file "$metrics" \
  --metrics.format prometheus \
  -c server-rs -t 3 -i 1 > "$stdout"
test -s "$metrics"
awk '$1 == "iperf3_bytes" && $2 > 0 { found=1 } END { exit found ? 0 : 1 }' "$metrics" || {
  echo "missing positive iperf3_bytes in metrics file" >&2
  cat "$metrics" >&2
  exit 1
}
awk '$1 == "iperf3_bandwidth" && $2 > 0 { found=1 } END { exit found ? 0 : 1 }' "$metrics" || {
  echo "missing positive iperf3_bandwidth in metrics file" >&2
  cat "$metrics" >&2
  exit 1
}
! grep -q '"event":"interval"' "$metrics"
cat "$stdout"
"#,
    ]);
    assert_success(
        &["iperf3-rs Prometheus metrics file and Pushgateway fan-out"],
        &file_output,
    );
    assert_human_iperf_output(&file_output);
    wait_for_pushgateway_metrics(
        &project,
        FILE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // Delete-on-exit check: require retained metrics to appear while the client
    // is still running, then require the same grouping key to disappear after
    // the process exits and sends DELETE to Pushgateway.
    let delete_args = [
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-delete",
        "--push.delete-on-exit",
        "-c",
        "server-rs",
        "-t",
        "6",
        "-i",
        "1",
    ];
    let mut delete_client = project.spawn_client(&delete_args);
    wait_for_pushgateway_metrics(
        &project,
        DELETE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert!(
        delete_client
            .try_wait()
            .expect("failed to poll delete-on-exit client")
            .is_none(),
        "delete-on-exit client exited before live metrics were observed"
    );
    assert_child_success(&delete_args, delete_client);
    wait_for_metric_absent(&project, "iperf3_bytes", DELETE_SCENARIO);
    assert_metric_absent(&project, "iperf3_bandwidth", DELETE_SCENARIO);

    // Window push check: `--push.interval` intentionally changes the exported
    // metric family names. The Pushgateway should retain representative window
    // summaries, and the same grouping key should not also receive immediate
    // interval gauges that would make the metric semantics ambiguous.
    let window_output = project.client_output(&[
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-window",
        "--push.interval",
        "2s",
        "-c",
        "server-rs",
        "-t",
        "3",
        "-i",
        "1",
    ]);
    assert_success(
        &["iperf3-rs window metrics-enabled human stdout"],
        &window_output,
    );
    assert_human_iperf_output(&window_output);
    wait_for_pushgateway_metrics(
        &project,
        WINDOW_SCENARIO,
        &[
            "iperf3_window_duration_seconds",
            "iperf3_window_transferred_bytes",
            "iperf3_window_bandwidth_mean_bytes_per_second",
            "iperf3_window_tcp_rtt_mean_seconds",
        ],
    );
    assert_metric_absent(&project, "iperf3_bandwidth", WINDOW_SCENARIO);

    // UDP uses different interval fields from TCP. Requiring UDP-prefixed packet
    // metrics in addition to bytes and bandwidth ensures the sender-side UDP
    // mapping is exercised by the Docker integration path.
    let udp = retry_json_client("iperf3-rs UDP client to iperf3-rs server", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=udp",
            "-c",
            "server-rs",
            "-u",
            "-b",
            "1M",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&udp);
    wait_for_pushgateway_metrics(
        &project,
        UDP_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth", "iperf3_udp_packets"],
    );
    assert_metric_value_eq(
        &project,
        "iperf3_udp_out_of_order_packets",
        UDP_SCENARIO,
        0.0,
    );
    // Jitter is measured by the UDP receiver, so it should appear on the
    // metrics-enabled server rather than on the sending client.
    wait_for_metric_value_gt(&project, "iperf3_udp_jitter_seconds", SERVER_SCENARIO, 0.0);
    assert_metric_value_eq(
        &project,
        "iperf3_udp_out_of_order_packets",
        SERVER_SCENARIO,
        0.0,
    );

    // Reverse mode flips the traffic direction while retaining the client
    // control path. It is a common iperf3 workflow and exercises a different
    // JSON shape from a plain sender-side TCP run.
    let reverse = retry_json_client("iperf3-rs reverse TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=tcp-reverse",
            "-c",
            "server-rs",
            "-R",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&reverse);
    wait_for_pushgateway_metrics(
        &project,
        REVERSE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // Bidirectional mode emits both forward and reverse stream data. Requiring
    // Pushgateway samples for it protects the interval aggregation path from
    // dropping bidirectional runs entirely.
    let bidir = retry_json_client("iperf3-rs bidirectional TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=tcp-bidir",
            "-c",
            "server-rs",
            "--bidir",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&bidir);
    wait_for_pushgateway_metrics(
        &project,
        BIDIR_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
}
