//! Reviewed adaptations of upstream CLI-only outcomes for the in-process API.
//! Inputs remain vendored source; only OUT_DIR copies are compiled differently.

pub fn api(source: &str) -> String {
    let source = replace_once(
        source,
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
    source
}

pub fn server(source: &str) -> String {
    let source = replace_once(
        source,
        "\t\t\t  exit(0);",
        "\t\t\t  return 0; /* one-off idle completion */",
    );
    assert!(!source.contains("exit("), "new upstream server exit path");
    source
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
