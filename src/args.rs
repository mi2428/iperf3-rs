use std::env;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use url::Url;

const DEFAULT_PUSH_JOB: &str = "iperf3";
const DEFAULT_PUSH_METRIC_PREFIX: &str = "iperf3";
const DEFAULT_PUSH_RETRIES: u32 = 0;
const MAX_PUSH_RETRIES: u32 = 10;

#[derive(Debug)]
pub struct AppOptions {
    pub push_url: Option<Url>,
    pub push_job: String,
    pub push_labels: Vec<(String, String)>,
    pub push_timeout: Duration,
    pub push_retries: u32,
    pub push_user_agent: String,
    pub push_metric_prefix: String,
    pub mirror_json: bool,
    pub show_help: bool,
    pub show_version: bool,
}

pub fn extract_app_options(args: Vec<String>) -> Result<(AppOptions, Vec<String>)> {
    extract_app_options_with_env(args, |key| env::var(key).ok())
}

fn extract_app_options_with_env(
    args: Vec<String>,
    mut get_env: impl FnMut(&str) -> Option<String>,
) -> Result<(AppOptions, Vec<String>)> {
    // iperf3-rs options are consumed here so libiperf receives an argv that still
    // looks like the upstream iperf3 CLI.
    let mut pass_through = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    let program = iter.next().ok_or_else(|| anyhow!("missing argv[0]"))?;
    pass_through.push(program);

    let rest: Vec<String> = iter.collect();
    let (show_help, show_version) = find_informational_request(&rest);
    if show_help || show_version {
        return Ok((
            AppOptions {
                push_url: None,
                push_job: DEFAULT_PUSH_JOB.to_owned(),
                push_labels: Vec::new(),
                push_timeout: default_push_timeout(),
                push_retries: DEFAULT_PUSH_RETRIES,
                push_user_agent: default_push_user_agent(),
                push_metric_prefix: DEFAULT_PUSH_METRIC_PREFIX.to_owned(),
                mirror_json: false,
                show_help,
                show_version,
            },
            pass_through,
        ));
    }

    let mut push_url = get_env("PUSH_URL");
    let mut push_job = get_env("PUSH_JOB").unwrap_or_else(|| DEFAULT_PUSH_JOB.to_owned());
    let mut push_labels = get_env("PUSH_LABELS")
        .map(|raw| parse_env_labels(&raw))
        .transpose()?
        .unwrap_or_default();
    let mut push_timeout = get_env("PUSH_TIMEOUT")
        .map(|raw| parse_duration_option("PUSH_TIMEOUT", &raw))
        .transpose()?
        .unwrap_or_else(default_push_timeout);
    let mut push_retries = get_env("PUSH_RETRIES")
        .map(|raw| parse_retries("PUSH_RETRIES", &raw))
        .transpose()?
        .unwrap_or(DEFAULT_PUSH_RETRIES);
    let mut push_user_agent = get_env("PUSH_USER_AGENT")
        .map(|raw| parse_user_agent("PUSH_USER_AGENT", &raw))
        .transpose()?
        .unwrap_or_else(default_push_user_agent);
    let mut push_metric_prefix = get_env("PUSH_METRIC_PREFIX")
        .map(|raw| parse_metric_prefix("PUSH_METRIC_PREFIX", &raw))
        .transpose()?
        .unwrap_or_else(|| DEFAULT_PUSH_METRIC_PREFIX.to_owned());
    let mut saw_push_job = false;
    let mut saw_push_label = !push_labels.is_empty();
    let mut saw_push_setting = false;
    let mut mirror_json = false;

    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if arg == "--" {
            // After `--`, every token belongs to libiperf exactly as written.
            pass_through.extend(rest[i..].iter().cloned());
            break;
        }

        if observes_json_output(arg) {
            mirror_json = true;
        }

        if let Some((key, value)) = split_long_value(arg) {
            match key {
                "--push.url" => push_url = Some(value.to_owned()),
                "--push.job" => {
                    push_job = value.to_owned();
                    saw_push_job = true;
                }
                "--push.label" => {
                    push_labels.push(parse_label(value)?);
                    saw_push_label = true;
                }
                "--push.timeout" => {
                    push_timeout = parse_duration_option("--push.timeout", value)?;
                    saw_push_setting = true;
                }
                "--push.retries" => {
                    push_retries = parse_retries("--push.retries", value)?;
                    saw_push_setting = true;
                }
                "--push.user-agent" => {
                    push_user_agent = parse_user_agent("--push.user-agent", value)?;
                    saw_push_setting = true;
                }
                "--push.metric-prefix" => {
                    push_metric_prefix = parse_metric_prefix("--push.metric-prefix", value)?;
                    saw_push_setting = true;
                }
                _ => pass_through.push(arg.clone()),
            }
            i += 1;
            continue;
        }

        match arg.as_str() {
            "--push.url" => {
                push_url = Some(take_value(&rest, &mut i, "--push.url")?);
            }
            "--push.job" => {
                push_job = take_value(&rest, &mut i, "--push.job")?;
                saw_push_job = true;
            }
            "--push.label" => {
                push_labels.push(parse_label(&take_value(&rest, &mut i, "--push.label")?)?);
                saw_push_label = true;
            }
            "--push.timeout" => {
                push_timeout = parse_duration_option(
                    "--push.timeout",
                    &take_value(&rest, &mut i, "--push.timeout")?,
                )?;
                saw_push_setting = true;
            }
            "--push.retries" => {
                push_retries = parse_retries(
                    "--push.retries",
                    &take_value(&rest, &mut i, "--push.retries")?,
                )?;
                saw_push_setting = true;
            }
            "--push.user-agent" => {
                push_user_agent = parse_user_agent(
                    "--push.user-agent",
                    &take_value(&rest, &mut i, "--push.user-agent")?,
                )?;
                saw_push_setting = true;
            }
            "--push.metric-prefix" => {
                push_metric_prefix = parse_metric_prefix(
                    "--push.metric-prefix",
                    &take_value(&rest, &mut i, "--push.metric-prefix")?,
                )?;
                saw_push_setting = true;
            }
            _ => {
                pass_through.push(arg.clone());
                i += 1;
            }
        }
    }

    let push_url = push_url.as_deref().map(parse_url).transpose()?;
    if push_url.is_none() && saw_push_job {
        bail!("--push.job requires --push.url or PUSH_URL");
    }
    if push_url.is_none() && saw_push_label {
        bail!("--push.label requires --push.url or PUSH_URL");
    }
    if push_url.is_none() && saw_push_setting {
        bail!("push settings require --push.url or PUSH_URL");
    }
    if push_url.is_some() && push_job.is_empty() {
        bail!("--push.job must not be empty when --push.url is set");
    }
    reject_duplicate_labels(&push_labels)?;

    Ok((
        AppOptions {
            push_url,
            push_job,
            push_labels,
            push_timeout,
            push_retries,
            push_user_agent,
            push_metric_prefix,
            mirror_json,
            show_help: false,
            show_version: false,
        },
        pass_through,
    ))
}

fn split_long_value(arg: &str) -> Option<(&str, &str)> {
    arg.split_once('=').filter(|(key, _)| key.starts_with("--"))
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| anyhow!("{option} requires a value"))?;
    *index += 1;
    Ok(value.clone())
}

fn parse_url(raw: &str) -> Result<Url> {
    // Keep local development terse: `localhost:9091` means the normal HTTP
    // Pushgateway endpoint unless a scheme is explicitly provided.
    let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    Url::parse(&with_scheme).map_err(|err| anyhow!("invalid --push.url URL: {err}"))
}

fn default_push_timeout() -> Duration {
    Duration::from_secs(5)
}

fn default_push_user_agent() -> String {
    format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

fn parse_duration_option(option: &str, raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("{option} must not be empty");
    }

    let duration = if let Some(number) = raw.strip_suffix("ms") {
        Duration::from_millis(parse_duration_number(option, raw, number)?)
    } else if let Some(number) = raw.strip_suffix('s') {
        Duration::from_secs(parse_duration_number(option, raw, number)?)
    } else if let Some(number) = raw.strip_suffix('m') {
        Duration::from_secs(
            parse_duration_number(option, raw, number)?
                .checked_mul(60)
                .ok_or_else(|| anyhow!("{option} is too large: {raw}"))?,
        )
    } else {
        Duration::from_secs(parse_duration_number(option, raw, raw)?)
    };

    if duration.is_zero() {
        bail!("{option} must be greater than zero");
    }
    Ok(duration)
}

fn parse_duration_number(option: &str, raw: &str, number: &str) -> Result<u64> {
    if number.is_empty() {
        bail!("invalid {option} duration: {raw}");
    }
    number
        .parse::<u64>()
        .map_err(|_| anyhow!("invalid {option} duration: {raw}"))
}

fn parse_retries(option: &str, raw: &str) -> Result<u32> {
    let retries = raw
        .trim()
        .parse::<u32>()
        .map_err(|_| anyhow!("{option} must be an integer between 0 and {MAX_PUSH_RETRIES}"))?;
    if retries > MAX_PUSH_RETRIES {
        bail!("{option} must be at most {MAX_PUSH_RETRIES}");
    }
    Ok(retries)
}

fn parse_user_agent(option: &str, raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("{option} must not be empty");
    }
    if value.chars().any(char::is_control) {
        bail!("{option} must not contain control characters");
    }
    Ok(value.to_owned())
}

fn parse_metric_prefix(option: &str, raw: &str) -> Result<String> {
    let value = raw.trim();
    if !is_valid_label_name(value) {
        bail!("invalid {option} metric prefix '{value}'");
    }
    Ok(value.to_owned())
}

fn parse_env_labels(raw: &str) -> Result<Vec<(String, String)>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    raw.split(',')
        .map(str::trim)
        .map(parse_label)
        .collect::<Result<Vec<_>>>()
}

fn parse_label(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("--push.label requires KEY=VALUE"))?;
    // Pushgateway grouping keys become Prometheus labels, so reject names that
    // would fail ingestion or conflict with labels managed by this wrapper.
    if !is_valid_label_name(name) {
        bail!("invalid --push.label name '{name}'");
    }
    if matches!(name, "job" | "iperf_mode") {
        bail!("--push.label name '{name}' is reserved");
    }
    if value.is_empty() {
        bail!("--push.label value for '{name}' must not be empty");
    }

    Ok((name.to_owned(), value.to_owned()))
}

fn is_valid_label_name(name: &str) -> bool {
    is_valid_label_name_bytes(name.as_bytes())
}

fn is_valid_label_name_bytes(name: &[u8]) -> bool {
    let Some((&first, rest)) = name.split_first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    for &byte in rest {
        if !(byte.is_ascii_alphanumeric() || byte == b'_') {
            return false;
        }
    }
    true
}

fn reject_duplicate_labels(labels: &[(String, String)]) -> Result<()> {
    for (index, (name, _)) in labels.iter().enumerate() {
        if labels[..index]
            .iter()
            .any(|(previous_name, _)| previous_name == name)
        {
            bail!("duplicate --push.label name '{name}'");
        }
    }
    Ok(())
}

fn observes_json_output(arg: &str) -> bool {
    arg == "-J" || arg == "--json" || arg == "--json-stream" || arg == "--json-stream-full-output"
}

fn find_informational_request(args: &[String]) -> (bool, bool) {
    let mut show_help = false;
    let mut show_version = false;
    for arg in args {
        if arg == "--" {
            break;
        }
        show_help |= is_help_option(arg);
        show_version |= is_version_option(arg);
    }
    (show_help, show_version)
}

fn is_version_option(arg: &str) -> bool {
    arg == "-v" || arg == "--version"
}

fn is_help_option(arg: &str) -> bool {
    arg == "-h" || arg == "--help"
}

#[cfg(kani)]
mod verification {
    use super::*;

    const MAX_LABEL_NAME_BYTES: usize = 4;

    #[kani::proof]
    #[kani::unwind(6)]
    fn valid_label_name_matches_prometheus_label_shape_for_bounded_ascii() {
        let len: usize = kani::any();
        kani::assume(len <= MAX_LABEL_NAME_BYTES);
        let bytes: [u8; MAX_LABEL_NAME_BYTES] = kani::any();

        let name = &bytes[..len];
        let expected = if let Some((&first, rest)) = name.split_first() {
            let mut ok = first.is_ascii_alphabetic() || first == b'_';
            for &byte in rest {
                ok &= byte.is_ascii_alphanumeric() || byte == b'_';
            }
            ok
        } else {
            false
        };

        assert_eq!(is_valid_label_name_bytes(name), expected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_custom_options() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
            "--push.url".to_owned(),
            "localhost:9091".to_owned(),
            "--push.job=net".to_owned(),
            "--push.label".to_owned(),
            "test=testrun".to_owned(),
            "--push.label=scenario=sample1".to_owned(),
            "--push.timeout=2s".to_owned(),
            "--push.retries".to_owned(),
            "2".to_owned(),
            "--push.user-agent=iperf3-rs/custom".to_owned(),
            "--push.metric-prefix".to_owned(),
            "nettest".to_owned(),
            "-t".to_owned(),
            "3".to_owned(),
        ];

        let (app, iperf) = extract_app_options(args).unwrap();
        assert_eq!(app.push_url.unwrap().as_str(), "http://localhost:9091/");
        assert_eq!(app.push_job, "net");
        assert_eq!(
            app.push_labels,
            [
                ("test".to_owned(), "testrun".to_owned()),
                ("scenario".to_owned(), "sample1".to_owned())
            ]
        );
        assert_eq!(app.push_timeout, Duration::from_secs(2));
        assert_eq!(app.push_retries, 2);
        assert_eq!(app.push_user_agent, "iperf3-rs/custom");
        assert_eq!(app.push_metric_prefix, "nettest");
        assert_eq!(iperf, ["iperf3-rs", "-c", "127.0.0.1", "-t", "3"]);
    }

    #[test]
    fn cli_values_override_environment_defaults() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "-s".to_owned(),
            "--push.url=http://cli.example:9091".to_owned(),
            "--push.job".to_owned(),
            "cli-job".to_owned(),
            "--push.label=site=tokyo".to_owned(),
        ];

        let (app, iperf) = extract_app_options_with_env(args, |key| match key {
            "PUSH_URL" => Some("http://env.example:9091".to_owned()),
            "PUSH_JOB" => Some("env-job".to_owned()),
            "PUSH_LABELS" => Some("test=env-test,scenario=env-scenario".to_owned()),
            _ => None,
        })
        .unwrap();

        assert_eq!(app.push_url.unwrap().as_str(), "http://cli.example:9091/");
        assert_eq!(app.push_job, "cli-job");
        assert_eq!(
            app.push_labels,
            [
                ("test".to_owned(), "env-test".to_owned()),
                ("scenario".to_owned(), "env-scenario".to_owned()),
                ("site".to_owned(), "tokyo".to_owned())
            ]
        );
        assert_eq!(iperf, ["iperf3-rs", "-s"]);
    }

    #[test]
    fn preserves_arguments_after_double_dash() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
            "--".to_owned(),
            "--push.url".to_owned(),
            "ignored-by-wrapper".to_owned(),
        ];

        let (app, iperf) = extract_app_options_with_env(args, |_| None).unwrap();
        assert!(app.push_url.is_none());
        assert_eq!(
            iperf,
            [
                "iperf3-rs",
                "-c",
                "127.0.0.1",
                "--",
                "--push.url",
                "ignored-by-wrapper"
            ]
        );
    }

    #[test]
    fn rejects_missing_custom_option_value() {
        for option in [
            "--push.url",
            "--push.job",
            "--push.label",
            "--push.timeout",
            "--push.retries",
            "--push.user-agent",
            "--push.metric-prefix",
        ] {
            let args = vec!["iperf3-rs".to_owned(), option.to_owned()];

            let err = extract_app_options_with_env(args, |_| None).unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("{option} requires a value")),
                "{option} should require a value"
            );
        }
    }

    #[test]
    fn rejects_empty_grouping_when_pushgateway_is_enabled() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.url".to_owned(),
            "localhost:9091".to_owned(),
            "--push.job=".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--push.job must not be empty when --push.url is set")
        );
    }

    #[test]
    fn rejects_push_labels_without_push_url() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.label".to_owned(),
            "test=testrun".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(err.to_string().contains("--push.label requires --push.url"));
    }

    #[test]
    fn rejects_malformed_push_label() {
        for label in [
            "missing-equals",
            "9bad=value",
            "job=value",
            "iperf_mode=client",
            "ok=",
        ] {
            let args = vec![
                "iperf3-rs".to_owned(),
                "--push.url".to_owned(),
                "localhost:9091".to_owned(),
                "--push.label".to_owned(),
                label.to_owned(),
            ];

            assert!(
                extract_app_options_with_env(args, |_| None).is_err(),
                "{label} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_duplicate_push_labels() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.url".to_owned(),
            "localhost:9091".to_owned(),
            "--push.label".to_owned(),
            "test=one".to_owned(),
            "--push.label".to_owned(),
            "test=two".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("duplicate --push.label name 'test'")
        );
    }

    #[test]
    fn parses_push_transport_and_metric_options_from_environment() {
        let args = vec!["iperf3-rs".to_owned(), "-s".to_owned()];

        let (app, iperf) = extract_app_options_with_env(args, |key| match key {
            "PUSH_URL" => Some("localhost:9091".to_owned()),
            "PUSH_TIMEOUT" => Some("500ms".to_owned()),
            "PUSH_RETRIES" => Some("3".to_owned()),
            "PUSH_USER_AGENT" => Some("iperf3-rs/env".to_owned()),
            "PUSH_METRIC_PREFIX" => Some("nettest".to_owned()),
            _ => None,
        })
        .unwrap();

        assert_eq!(app.push_url.unwrap().as_str(), "http://localhost:9091/");
        assert_eq!(app.push_timeout, Duration::from_millis(500));
        assert_eq!(app.push_retries, 3);
        assert_eq!(app.push_user_agent, "iperf3-rs/env");
        assert_eq!(app.push_metric_prefix, "nettest");
        assert_eq!(iperf, ["iperf3-rs", "-s"]);
    }

    #[test]
    fn rejects_push_settings_without_push_url() {
        for option in [
            "--push.timeout=5s",
            "--push.retries=1",
            "--push.user-agent=iperf3-rs/test",
            "--push.metric-prefix=nettest",
        ] {
            let args = vec!["iperf3-rs".to_owned(), option.to_owned()];

            let err = extract_app_options_with_env(args, |_| None).unwrap_err();
            assert!(
                err.to_string()
                    .contains("push settings require --push.url or PUSH_URL"),
                "{option} should require Pushgateway to be enabled"
            );
        }
    }

    #[test]
    fn rejects_malformed_push_transport_and_metric_options() {
        for (option, value, expected) in [
            (
                "--push.timeout",
                "0",
                "--push.timeout must be greater than zero",
            ),
            (
                "--push.timeout",
                "1h",
                "invalid --push.timeout duration: 1h",
            ),
            ("--push.retries", "11", "--push.retries must be at most 10"),
            (
                "--push.user-agent",
                "",
                "--push.user-agent must not be empty",
            ),
            (
                "--push.metric-prefix",
                "bad-prefix",
                "invalid --push.metric-prefix metric prefix",
            ),
        ] {
            let args = vec![
                "iperf3-rs".to_owned(),
                "--push.url".to_owned(),
                "localhost:9091".to_owned(),
                option.to_owned(),
                value.to_owned(),
            ];

            let err = extract_app_options_with_env(args, |_| None).unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "{option}={value:?} should fail with {expected:?}, got {err:#}"
            );
        }
    }

    #[test]
    fn parses_push_timeout_units() {
        assert_eq!(
            parse_duration_option("--push.timeout", "500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_duration_option("--push.timeout", "5s").unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_duration_option("--push.timeout", "1m").unwrap(),
            Duration::from_secs(60)
        );
        assert_eq!(
            parse_duration_option("--push.timeout", "7").unwrap(),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn notices_user_requested_json_output() {
        for flag in ["-J", "--json", "--json-stream", "--json-stream-full-output"] {
            let args = vec!["iperf3-rs".to_owned(), flag.to_owned()];
            let (app, _) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(app.mirror_json, "{flag} should mirror JSON output");
        }
    }

    #[test]
    fn strips_version_options() {
        for flag in ["-v", "--version"] {
            let args = vec![
                "iperf3-rs".to_owned(),
                flag.to_owned(),
                "-c".to_owned(),
                "127.0.0.1".to_owned(),
            ];

            let (app, iperf) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(
                app.show_version,
                "{flag} should request wrapper version output"
            );
            assert_eq!(iperf, ["iperf3-rs"]);
        }
    }

    #[test]
    fn strips_help_options() {
        for flag in ["-h", "--help"] {
            let args = vec![
                "iperf3-rs".to_owned(),
                flag.to_owned(),
                "-c".to_owned(),
                "127.0.0.1".to_owned(),
            ];

            let (app, iperf) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(app.show_help, "{flag} should request wrapper help output");
            assert_eq!(iperf, ["iperf3-rs"]);
        }
    }

    #[test]
    fn version_request_skips_pushgateway_consistency_checks() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--version".to_owned(),
            "--push.label".to_owned(),
            "scenario=ignored".to_owned(),
        ];

        let (app, _) = extract_app_options_with_env(args, |_| None).unwrap();
        assert!(app.show_version);
    }

    #[test]
    fn help_request_skips_pushgateway_consistency_checks() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--help".to_owned(),
            "--push.job".to_owned(),
            "ignored".to_owned(),
        ];

        let (app, _) = extract_app_options_with_env(args, |_| None).unwrap();
        assert!(app.show_help);
    }

    #[test]
    fn informational_requests_ignore_malformed_pushgateway_environment() {
        for flag in ["--help", "--version"] {
            let args = vec!["iperf3-rs".to_owned(), flag.to_owned()];

            let (app, _) = extract_app_options_with_env(args, |key| match key {
                "PUSH_LABELS" => Some("not-a-label".to_owned()),
                "PUSH_TIMEOUT" => Some("not-a-duration".to_owned()),
                "PUSH_RETRIES" => Some("not-a-number".to_owned()),
                "PUSH_METRIC_PREFIX" => Some("bad-prefix".to_owned()),
                _ => None,
            })
            .unwrap();

            assert!(app.show_help || app.show_version);
        }
    }
}
