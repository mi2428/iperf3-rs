//! Reviewed adaptations of upstream CLI-only outcomes for the in-process API.
//! Inputs remain vendored source; only OUT_DIR copies are compiled differently.

pub fn api(source: &str) -> String {
    let source = replace_once(
        source,
        "#include \"iperf_api.h\"",
        "#include \"iperf_api.h\"\n#include \"iperf3rs_cli.h\"",
    );
    let source = replace_once(
        &source,
        "            case 'v':\n                printf(\"%s (cJSON %s)\\n%s\\n%s\\n\", version, cJSON_Version(), get_system_info(),\n\t\t       get_optional_features());\n                exit(0);",
        "            case 'v':\n                return 2; /* version outcome, not process exit */",
    );
    let source = replace_once(
        &source,
        "\t    case 'h':\n\t\tusage_long(stdout);\n\t\texit(0);",
        "\t    case 'h':\n\t\treturn 1; /* help outcome, not process exit */",
    );
    let source = replace_once(
        &source,
        "                usage();\n                exit(1);",
        "                usage();\n                return 3; /* invalid or missing option */",
    );
    let parser = source
        .split_once("int\niperf_parse_arguments(")
        .expect("upstream parser anchor changed")
        .1
        .split_once("\nint iperf_open_logfile(")
        .expect("upstream parser end anchor changed")
        .0;
    assert!(!parser.contains("exit("), "new upstream parser exit path");
    let cleanup = r#"static void
iperf3rs_delete_unattached(cJSON *parent, const char *name, cJSON *child)
{
    if (child != NULL && (parent == NULL || cJSON_GetObjectItem(parent, name) != child))
        cJSON_Delete(child);
}

static void
iperf3rs_release_json(struct iperf_test *test)
{
    iperf3rs_delete_unattached(test->json_start, "connected", test->json_connected);
    iperf3rs_delete_unattached(test->json_top, "start", test->json_start);
    iperf3rs_delete_unattached(test->json_top, "intervals", test->json_intervals);
    iperf3rs_delete_unattached(test->json_top, "end", test->json_end);
    iperf3rs_delete_unattached(test->json_top, "server_output_json", test->json_server_output);
    cJSON_Delete(test->json_top);
    test->json_top = test->json_start = test->json_connected = test->json_intervals = test->json_end = test->json_server_output = NULL;
    free(test->json_output_string);
    test->json_output_string = NULL;
    free(test->server_output_text);
    test->server_output_text = NULL;
}

void
iperf_free_test(struct iperf_test *test)
{
    iperf3rs_release_json(test);"#;
    let source = replace_once(
        &source,
        "void\niperf_free_test(struct iperf_test *test)\n{",
        cleanup,
    );
    let source = replace_once(
        &source,
        "void\niperf_reset_test(struct iperf_test *test)\n{",
        "void\niperf_reset_test(struct iperf_test *test)\n{\n    iperf3rs_release_json(test);",
    );
    let source = replace_once(
        &source,
        "            test->json_output_string = strdup(str);",
        "            free(test->json_output_string);\n            test->json_output_string = strdup(str);",
    );
    let pid_start = "int\niperf_create_pidfile(struct iperf_test *test)\n{";
    let pid_end = "/* Get rid of a PID file, return -1 on error. */";
    assert_eq!(
        source.matches(pid_start).count(),
        1,
        "upstream pidfile start changed"
    );
    assert_eq!(
        source.matches(pid_end).count(),
        1,
        "upstream pidfile end changed"
    );
    let (before, body) = source.split_once(pid_start).unwrap();
    let (_, after) = body.split_once(pid_end).unwrap();
    let source = format!(
        "{before}{pid_start}\n    int owned;\n    struct stat identity;\n    return iperf3rs_create_pidfile(test, &owned, &identity);\n}}\n\n{pid_end}{after}"
    );
    let source = replace_once(
        &source,
        "    if (test->server_hostname)\n\tfree(test->server_hostname);",
        "    free(test->pidfile);\n    test->pidfile = NULL;\n    if (test->server_hostname)\n\tfree(test->server_hostname);",
    );
    // Reuse upstream's partial-statistics and peer-notification sequence. The
    // owner calls this normally after a wakeup, never from the signal handler.
    let sig_start = "void\niperf_got_sigend(struct iperf_test *test, int sig)\n{";
    let body = source
        .split_once(sig_start)
        .expect("upstream sigend anchor changed")
        .1
        .split_once("\n    exit_normal = 0;")
        .expect("upstream sigend end changed")
        .0;
    let body = replace_once(body, "    int exit_normal;", "");
    let body = replace_once(
        &body,
        "\ttest->reporter_callback(test);",
        "\tif (test->role == 's') test->reporter_callback(test);",
    );
    let report = format!(
        "void\niperf3rs_cli_report_interrupt(struct iperf_test *test)\n{{{body}\n    if (test->role == 'c') iperf_set_test_state(test, DISPLAY_RESULTS);\n}}\n\n{sig_start}"
    );
    replace_once(&source, sig_start, &report)
}

pub fn server(source: &str) -> String {
    let source = replace_once(
        source,
        "#include \"iperf_api.h\"",
        "#include \"iperf_api.h\"\n#include \"iperf3rs_cli.h\"",
    );
    let source = replace_once(
        &source,
        "\t\t\t  exit(0);",
        "\t\t\t  return 0; /* one-off idle completion */",
    );
    assert!(!source.contains("exit("), "new upstream server exit path");
    replace_once(
        &source,
        "        result = select(test->max_fd + 1, &read_set, &write_set, NULL, timeout);",
        "        result = iperf3rs_cli_select(test->max_fd + 1, &read_set, &write_set, NULL, timeout);\n        if (iperf3rs_cli_interrupted()) {\n            iperf3rs_cli_report_interrupt(test);\n            cleanup_server(test);\n            return -2;\n        }",
    )
}

pub fn client(source: &str) -> String {
    let source = replace_once(
        source,
        "#include \"iperf_api.h\"",
        "#include \"iperf_api.h\"\n#include \"iperf3rs_cli.h\"",
    );
    assert_eq!(
        source.matches("result = select(").count(),
        2,
        "upstream client selects changed"
    );
    let source = source.replace("result = select(", "result = iperf3rs_cli_select(");
    let source = replace_once(
        &source,
        "#endif // __vxworks or __VXWORKS__\n\tif (result < 0 && errno != EINTR)",
        "#endif // __vxworks or __VXWORKS__\n        if (iperf3rs_cli_interrupted()) goto cleanup_and_fail;\n\tif (result < 0 && errno != EINTR)",
    );
    let source = replace_once(
        &source,
        "  cleanup_and_fail:\n    /* Cancel all outstanding threads */",
        "  cleanup_and_fail:\n    if (iperf3rs_cli_interrupted()) iperf3rs_cli_report_interrupt(test);\n    /* Cancel all outstanding threads */",
    );
    replace_once(
        &source,
        "        cJSON_AddStringToObject(test->json_top, \"error\", iperf_strerror(i_errno));",
        "        if (iperf3rs_cli_interrupted())\n            iperf_err(test, \"interrupt - %s by signal %s(%d)\", iperf_strerror(i_errno), strsignal(iperf3rs_cli_interrupted()), iperf3rs_cli_interrupted());\n        else\n            cJSON_AddStringToObject(test->json_top, \"error\", iperf_strerror(i_errno));",
    )
}

pub fn net(source: &str) -> String {
    let source = replace_once(
        source,
        "#include \"timer.h\"",
        "#include \"timer.h\"\n#include \"iperf3rs_cli.h\"",
    );
    assert_eq!(
        source.matches("r = select(").count(),
        2,
        "upstream control selects changed"
    );
    source.replace("r = select(", "r = iperf3rs_cli_select(")
}

pub fn option_metadata(source: &str) -> String {
    let table_start = "    static struct option longopts[] =\n    {\n";
    let table_end = "\n    };\n    int flag;";
    assert_eq!(
        source.matches(table_start).count(),
        1,
        "upstream option table anchor changed"
    );
    let table = source
        .split_once(table_start)
        .unwrap()
        .1
        .split_once(table_end)
        .expect("upstream option table end changed")
        .0;
    let short_start = "getopt_long(argc, argv, \"";
    let short_end = "\", longopts, NULL)) != -1)";
    assert_eq!(
        source.matches(short_start).count(),
        1,
        "upstream short option anchor changed"
    );
    let short = source
        .split_once(short_start)
        .unwrap()
        .1
        .split_once(short_end)
        .expect("upstream short option end changed")
        .0;
    format!(
        "/* Generated verbatim from upstream iperf_parse_arguments. */\nstatic const struct option iperf3rs_longopts[] = {{\n{table}\n}};\nstatic const char iperf3rs_shortopts[] = \"{short}\";\n"
    )
}

pub fn replace_once(source: &str, from: &str, to: &str) -> String {
    assert_eq!(
        source.matches(from).count(),
        1,
        "upstream adaptation anchor changed: {from}"
    );
    source.replacen(from, to, 1)
}
