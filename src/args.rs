use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use url::Url;

use crate::metrics_file::MetricsFileFormat;
use crate::prometheus::validate_metric_prefix;
use crate::pushgateway::{
    PushGatewayConfig, is_reserved_label_name, is_valid_label_name, validate_retries,
    validate_user_agent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurationUnit {
    Milliseconds,
    Seconds,
    Minutes,
}

#[derive(Debug)]
pub struct AppOptions {
    pub push_url: Option<Url>,
    pub push_job: String,
    pub push_labels: Vec<(String, String)>,
    pub push_timeout: Duration,
    pub push_retries: u32,
    pub push_user_agent: String,
    pub metrics_prefix: String,
    pub push_interval: Option<Duration>,
    pub push_delete_on_exit: bool,
    pub metrics_file: Option<PathBuf>,
    pub metrics_format: MetricsFileFormat,
    pub metrics_labels: Vec<(String, String)>,
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
                push_job: PushGatewayConfig::DEFAULT_JOB.to_owned(),
                push_labels: Vec::new(),
                push_timeout: PushGatewayConfig::default_timeout(),
                push_retries: PushGatewayConfig::DEFAULT_RETRIES,
                push_user_agent: PushGatewayConfig::default_user_agent(),
                metrics_prefix: PushGatewayConfig::DEFAULT_METRIC_PREFIX.to_owned(),
                push_interval: None,
                push_delete_on_exit: false,
                metrics_file: None,
                metrics_format: MetricsFileFormat::Jsonl,
                metrics_labels: Vec::new(),
                show_help,
                show_version,
            },
            pass_through,
        ));
    }

    let mut push_url = get_env("IPERF3_PUSH_URL");
    let mut push_job =
        get_env("IPERF3_PUSH_JOB").unwrap_or_else(|| PushGatewayConfig::DEFAULT_JOB.to_owned());
    let mut push_labels = get_env("IPERF3_PUSH_LABELS")
        .map(|raw| parse_env_labels("IPERF3_PUSH_LABELS", &raw, true))
        .transpose()?
        .unwrap_or_default();
    let mut push_timeout = get_env("IPERF3_PUSH_TIMEOUT")
        .map(|raw| parse_duration_option("IPERF3_PUSH_TIMEOUT", &raw))
        .transpose()?
        .unwrap_or_else(PushGatewayConfig::default_timeout);
    let mut push_retries = get_env("IPERF3_PUSH_RETRIES")
        .map(|raw| parse_retries("IPERF3_PUSH_RETRIES", &raw))
        .transpose()?
        .unwrap_or(PushGatewayConfig::DEFAULT_RETRIES);
    let mut push_user_agent = get_env("IPERF3_PUSH_USER_AGENT")
        .map(|raw| parse_user_agent("IPERF3_PUSH_USER_AGENT", &raw))
        .transpose()?
        .unwrap_or_else(PushGatewayConfig::default_user_agent);
    let mut metrics_prefix = get_env("IPERF3_METRICS_PREFIX")
        .map(|raw| parse_metric_prefix("IPERF3_METRICS_PREFIX", &raw))
        .transpose()?
        .unwrap_or_else(|| PushGatewayConfig::DEFAULT_METRIC_PREFIX.to_owned());
    let mut push_interval = get_env("IPERF3_PUSH_INTERVAL")
        .map(|raw| parse_duration_option("IPERF3_PUSH_INTERVAL", &raw))
        .transpose()?;
    let mut push_delete_on_exit = get_env("IPERF3_PUSH_DELETE_ON_EXIT")
        .map(|raw| parse_bool_option("IPERF3_PUSH_DELETE_ON_EXIT", &raw))
        .transpose()?
        .unwrap_or(false);
    let mut metrics_file = get_env("IPERF3_METRICS_FILE").map(PathBuf::from);
    let raw_metrics_format = get_env("IPERF3_METRICS_FORMAT");
    let mut metrics_format = raw_metrics_format
        .as_deref()
        .map(|raw| parse_metrics_format("IPERF3_METRICS_FORMAT", raw))
        .transpose()?
        .unwrap_or(MetricsFileFormat::Jsonl);
    let mut metrics_labels = get_env("IPERF3_METRICS_LABELS")
        .map(|raw| parse_env_labels("IPERF3_METRICS_LABELS", &raw, false))
        .transpose()?
        .unwrap_or_default();
    let mut saw_push_job = false;
    let mut saw_push_label = !push_labels.is_empty();
    let mut saw_push_setting = false;
    let mut saw_metrics_setting = raw_metrics_format.is_some();
    let mut saw_metrics_label = !metrics_labels.is_empty();
    let mut saw_metric_prefix = false;

    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if arg == "--" {
            // After `--`, every token belongs to libiperf exactly as written.
            pass_through.extend(rest[i..].iter().cloned());
            break;
        }

        if let Some((key, value)) = split_long_value(arg) {
            match key {
                "--push.url" => push_url = Some(value.to_owned()),
                "--push.job" => {
                    push_job = value.to_owned();
                    saw_push_job = true;
                }
                "--push.label" => {
                    push_labels.push(parse_label("--push.label", value, true)?);
                    saw_push_label = true;
                }
                "--metrics.label" => {
                    metrics_labels.push(parse_label("--metrics.label", value, false)?);
                    saw_metrics_label = true;
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
                "--metrics.prefix" => {
                    metrics_prefix = parse_metric_prefix("--metrics.prefix", value)?;
                    saw_metric_prefix = true;
                }
                "--push.interval" => {
                    push_interval = Some(parse_duration_option("--push.interval", value)?);
                    saw_push_setting = true;
                }
                "--push.delete-on-exit" => {
                    push_delete_on_exit = parse_bool_option("--push.delete-on-exit", value)?;
                    saw_push_setting = true;
                }
                "--metrics.file" => {
                    metrics_file = Some(PathBuf::from(value));
                }
                "--metrics.format" => {
                    metrics_format = parse_metrics_format("--metrics.format", value)?;
                    saw_metrics_setting = true;
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
                push_labels.push(parse_label(
                    "--push.label",
                    &take_value(&rest, &mut i, "--push.label")?,
                    true,
                )?);
                saw_push_label = true;
            }
            "--metrics.label" => {
                metrics_labels.push(parse_label(
                    "--metrics.label",
                    &take_value(&rest, &mut i, "--metrics.label")?,
                    false,
                )?);
                saw_metrics_label = true;
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
            "--metrics.prefix" => {
                metrics_prefix = parse_metric_prefix(
                    "--metrics.prefix",
                    &take_value(&rest, &mut i, "--metrics.prefix")?,
                )?;
                saw_metric_prefix = true;
            }
            "--push.interval" => {
                push_interval = Some(parse_duration_option(
                    "--push.interval",
                    &take_value(&rest, &mut i, "--push.interval")?,
                )?);
                saw_push_setting = true;
            }
            "--push.delete-on-exit" => {
                push_delete_on_exit = true;
                saw_push_setting = true;
                i += 1;
            }
            "--metrics.file" => {
                metrics_file = Some(PathBuf::from(take_value(&rest, &mut i, "--metrics.file")?));
            }
            "--metrics.format" => {
                metrics_format = parse_metrics_format(
                    "--metrics.format",
                    &take_value(&rest, &mut i, "--metrics.format")?,
                )?;
                saw_metrics_setting = true;
            }
            _ => {
                pass_through.push(arg.clone());
                i += 1;
            }
        }
    }

    let push_url = push_url.as_deref().map(parse_url).transpose()?;
    if push_url.is_none() && saw_push_job {
        bail!("--push.job requires --push.url or IPERF3_PUSH_URL");
    }
    if push_url.is_none() && saw_push_label {
        bail!("--push.label requires --push.url or IPERF3_PUSH_URL");
    }
    if push_url.is_none() && saw_push_setting {
        bail!("push settings require --push.url or IPERF3_PUSH_URL");
    }
    if metrics_file.is_none() && saw_metrics_setting {
        bail!("metrics settings require --metrics.file or IPERF3_METRICS_FILE");
    }
    if metrics_file.is_none() && saw_metrics_label {
        bail!("--metrics.label requires --metrics.file or IPERF3_METRICS_FILE");
    }
    if saw_metrics_label && metrics_format != MetricsFileFormat::Prometheus {
        bail!("--metrics.label requires --metrics.format prometheus");
    }
    if push_url.is_none() && metrics_file.is_none() && saw_metric_prefix {
        bail!(
            "metric prefix requires --metrics.file, IPERF3_METRICS_FILE, --push.url, or IPERF3_PUSH_URL"
        );
    }
    if push_url.is_some() && push_job.is_empty() {
        bail!("--push.job must not be empty when --push.url is set");
    }
    reject_duplicate_labels("--push.label", &push_labels)?;
    reject_duplicate_labels("--metrics.label", &metrics_labels)?;

    Ok((
        AppOptions {
            push_url,
            push_job,
            push_labels,
            push_timeout,
            push_retries,
            push_user_agent,
            metrics_prefix,
            push_interval,
            push_delete_on_exit,
            metrics_file,
            metrics_format,
            metrics_labels,
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
    PushGatewayConfig::parse_endpoint(raw).map_err(|err| anyhow!("invalid --push.url URL: {err}"))
}

fn parse_duration_option(option: &str, raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("{option} must not be empty");
    }

    let duration = if let Some(number) = raw.strip_suffix("ms") {
        duration_from_number(
            parse_duration_number(option, raw, number)?,
            DurationUnit::Milliseconds,
        )
        .expect("millisecond durations cannot overflow")
    } else if let Some(number) = raw.strip_suffix('s') {
        duration_from_number(
            parse_duration_number(option, raw, number)?,
            DurationUnit::Seconds,
        )
        .expect("second durations cannot overflow")
    } else if let Some(number) = raw.strip_suffix('m') {
        duration_from_number(
            parse_duration_number(option, raw, number)?,
            DurationUnit::Minutes,
        )
        .ok_or_else(|| anyhow!("{option} is too large: {raw}"))?
    } else {
        duration_from_number(
            parse_duration_number(option, raw, raw)?,
            DurationUnit::Seconds,
        )
        .expect("second durations cannot overflow")
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

fn duration_from_number(number: u64, unit: DurationUnit) -> Option<Duration> {
    match unit {
        DurationUnit::Milliseconds => Some(Duration::from_millis(number)),
        DurationUnit::Seconds => Some(Duration::from_secs(number)),
        DurationUnit::Minutes => number.checked_mul(60).map(Duration::from_secs),
    }
}

fn parse_retries(option: &str, raw: &str) -> Result<u32> {
    let retries = raw.trim().parse::<u32>().map_err(|_| {
        anyhow!(
            "{option} must be an integer between 0 and {}",
            PushGatewayConfig::MAX_RETRIES
        )
    })?;
    validate_retries(retries).map_err(|_| {
        anyhow!(
            "{option} must be at most {}",
            PushGatewayConfig::MAX_RETRIES
        )
    })?;
    Ok(retries)
}

fn parse_bool_option(option: &str, raw: &str) -> Result<bool> {
    parse_bool_literal(raw.trim())
        .ok_or_else(|| anyhow!("{option} must be one of true, false, 1, 0, yes, no, on, or off"))
}

fn parse_bool_literal(raw: &str) -> Option<bool> {
    parse_bool_literal_bytes(raw.as_bytes())
}

fn parse_bool_literal_bytes(raw: &[u8]) -> Option<bool> {
    if bytes_eq_ignore_ascii_case(raw, b"1")
        || bytes_eq_ignore_ascii_case(raw, b"true")
        || bytes_eq_ignore_ascii_case(raw, b"yes")
        || bytes_eq_ignore_ascii_case(raw, b"on")
    {
        return Some(true);
    }
    if bytes_eq_ignore_ascii_case(raw, b"0")
        || bytes_eq_ignore_ascii_case(raw, b"false")
        || bytes_eq_ignore_ascii_case(raw, b"no")
        || bytes_eq_ignore_ascii_case(raw, b"off")
    {
        return Some(false);
    }
    None
}

fn bytes_eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn parse_metrics_format(option: &str, raw: &str) -> Result<MetricsFileFormat> {
    MetricsFileFormat::parse(raw)
        .ok_or_else(|| anyhow!("{option} must be one of jsonl or prometheus"))
}

#[cfg(kani)]
fn is_valid_retry_count(retries: u32) -> bool {
    retries <= PushGatewayConfig::MAX_RETRIES
}

fn parse_user_agent(option: &str, raw: &str) -> Result<String> {
    let value = raw.trim();
    validate_user_agent(value).map_err(|err| {
        anyhow!(
            "{}",
            err.to_string().replace("Pushgateway User-Agent", option)
        )
    })?;
    Ok(value.to_owned())
}

fn parse_metric_prefix(option: &str, raw: &str) -> Result<String> {
    let value = raw.trim();
    validate_metric_prefix(value)
        .map_err(|_| anyhow!("invalid {option} metric prefix '{value}'"))?;
    Ok(value.to_owned())
}

fn parse_env_labels(option: &str, raw: &str, reserve_job: bool) -> Result<Vec<(String, String)>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    raw.split(',')
        .map(str::trim)
        .map(|label| parse_label(option, label, reserve_job))
        .collect::<Result<Vec<_>>>()
}

fn parse_label(option: &str, raw: &str, reserve_job: bool) -> Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("{option} requires KEY=VALUE"))?;
    if !is_valid_label_name(name) {
        bail!("invalid {option} name '{name}'");
    }
    if reserve_job && is_reserved_label_name(name) {
        bail!("{option} name '{name}' is reserved");
    }
    if value.is_empty() {
        bail!("{option} value for '{name}' must not be empty");
    }

    Ok((name.to_owned(), value.to_owned()))
}

fn reject_duplicate_labels(option: &str, labels: &[(String, String)]) -> Result<()> {
    for (index, (name, _)) in labels.iter().enumerate() {
        if labels[..index]
            .iter()
            .any(|(previous_name, _)| previous_name == name)
        {
            bail!("duplicate {option} name '{name}'");
        }
    }
    Ok(())
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
    use crate::pushgateway::{is_reserved_label_name_bytes, is_valid_label_name_bytes};

    use super::*;

    const MAX_LABEL_NAME_BYTES: usize = 4;
    const MAX_RESERVED_LABEL_NAME_BYTES: usize = 3;

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

    #[kani::proof]
    #[kani::unwind(5)]
    fn reserved_label_name_matches_reserved_grouping_key_for_bounded_ascii() {
        let len: usize = kani::any();
        kani::assume(len <= MAX_RESERVED_LABEL_NAME_BYTES);
        let bytes: [u8; MAX_RESERVED_LABEL_NAME_BYTES] = kani::any();

        let name = &bytes[..len];
        let expected = name == b"job";

        assert_eq!(is_reserved_label_name_bytes(name), expected);
    }

    #[kani::proof]
    fn duration_from_small_number_matches_unit_arithmetic() {
        let number: u16 = kani::any();
        let number = u64::from(number);

        assert_eq!(
            duration_from_number(number, DurationUnit::Milliseconds),
            Some(Duration::from_millis(number))
        );
        assert_eq!(
            duration_from_number(number, DurationUnit::Seconds),
            Some(Duration::from_secs(number))
        );

        let minutes = duration_from_number(number, DurationUnit::Minutes);
        assert_eq!(minutes, Some(Duration::from_secs(number * 60)));
    }

    #[kani::proof]
    fn minute_duration_rejects_multiplication_overflow() {
        let number: u64 = kani::any();
        kani::assume(number > u64::MAX / 60);

        assert!(duration_from_number(number, DurationUnit::Minutes).is_none());
    }

    #[kani::proof]
    fn retry_count_acceptance_matches_configured_limit() {
        let retries: u32 = kani::any();

        assert_eq!(
            is_valid_retry_count(retries),
            retries <= PushGatewayConfig::MAX_RETRIES
        );
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn bool_literal_parser_matches_documented_values_for_bounded_bytes() {
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let bytes: [u8; 5] = kani::any();
        let raw = &bytes[..len];

        let expected_true = bytes_eq_ignore_ascii_case(raw, b"1")
            || bytes_eq_ignore_ascii_case(raw, b"true")
            || bytes_eq_ignore_ascii_case(raw, b"yes")
            || bytes_eq_ignore_ascii_case(raw, b"on");
        let expected_false = bytes_eq_ignore_ascii_case(raw, b"0")
            || bytes_eq_ignore_ascii_case(raw, b"false")
            || bytes_eq_ignore_ascii_case(raw, b"no")
            || bytes_eq_ignore_ascii_case(raw, b"off");
        let expected = if expected_true {
            Some(true)
        } else if expected_false {
            Some(false)
        } else {
            None
        };

        assert_eq!(parse_bool_literal_bytes(raw), expected);
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
            "--push.label=mode=client".to_owned(),
            "--push.timeout=2s".to_owned(),
            "--push.retries".to_owned(),
            "2".to_owned(),
            "--push.user-agent=iperf3-rs/custom".to_owned(),
            "--metrics.prefix".to_owned(),
            "nettest".to_owned(),
            "--push.interval=10s".to_owned(),
            "--push.delete-on-exit".to_owned(),
            "--metrics.file".to_owned(),
            "metrics.jsonl".to_owned(),
            "--metrics.format=prometheus".to_owned(),
            "--metrics.label".to_owned(),
            "site=ci".to_owned(),
            "--metrics.label=run=nightly".to_owned(),
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
                ("scenario".to_owned(), "sample1".to_owned()),
                ("mode".to_owned(), "client".to_owned())
            ]
        );
        assert_eq!(app.push_timeout, Duration::from_secs(2));
        assert_eq!(app.push_retries, 2);
        assert_eq!(app.push_user_agent, "iperf3-rs/custom");
        assert_eq!(app.metrics_prefix, "nettest");
        assert_eq!(app.push_interval, Some(Duration::from_secs(10)));
        assert!(app.push_delete_on_exit);
        assert_eq!(app.metrics_file, Some(PathBuf::from("metrics.jsonl")));
        assert_eq!(app.metrics_format, MetricsFileFormat::Prometheus);
        assert_eq!(
            app.metrics_labels,
            [
                ("site".to_owned(), "ci".to_owned()),
                ("run".to_owned(), "nightly".to_owned())
            ]
        );
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
            "IPERF3_PUSH_URL" => Some("http://env.example:9091".to_owned()),
            "IPERF3_PUSH_JOB" => Some("env-job".to_owned()),
            "IPERF3_PUSH_LABELS" => Some("test=env-test,scenario=env-scenario".to_owned()),
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
    fn unprefixed_environment_names_are_ignored() {
        let args = vec!["iperf3-rs".to_owned(), "-s".to_owned()];

        let (app, iperf) = extract_app_options_with_env(args, |key| match key {
            "PUSH_URL" => Some("http://env.example:9091".to_owned()),
            "PUSH_JOB" => Some("env-job".to_owned()),
            "METRICS_FILE" => Some("metrics.jsonl".to_owned()),
            "METRICS_PREFIX" => Some("nettest".to_owned()),
            _ => None,
        })
        .unwrap();

        assert!(app.push_url.is_none());
        assert_eq!(app.push_job, PushGatewayConfig::DEFAULT_JOB);
        assert!(app.metrics_file.is_none());
        assert_eq!(app.metrics_prefix, PushGatewayConfig::DEFAULT_METRIC_PREFIX);
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
            "--push.interval",
            "--metrics.file",
            "--metrics.format",
            "--metrics.label",
            "--metrics.prefix",
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
        for label in ["missing-equals", "9bad=value", "job=value", "ok="] {
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
    fn rejects_malformed_metrics_label() {
        for label in ["missing-equals", "9bad=value", "ok="] {
            let args = vec![
                "iperf3-rs".to_owned(),
                "--metrics.file".to_owned(),
                "metrics.prom".to_owned(),
                "--metrics.format=prometheus".to_owned(),
                "--metrics.label".to_owned(),
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
    fn rejects_duplicate_metrics_labels() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--metrics.file=metrics.prom".to_owned(),
            "--metrics.format=prometheus".to_owned(),
            "--metrics.label".to_owned(),
            "site=one".to_owned(),
            "--metrics.label".to_owned(),
            "site=two".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("duplicate --metrics.label name 'site'")
        );
    }

    #[test]
    fn parses_push_transport_and_metric_options_from_environment() {
        let args = vec!["iperf3-rs".to_owned(), "-s".to_owned()];

        let (app, iperf) = extract_app_options_with_env(args, |key| match key {
            "IPERF3_PUSH_URL" => Some("localhost:9091".to_owned()),
            "IPERF3_PUSH_TIMEOUT" => Some("500ms".to_owned()),
            "IPERF3_PUSH_RETRIES" => Some("3".to_owned()),
            "IPERF3_PUSH_USER_AGENT" => Some("iperf3-rs/env".to_owned()),
            "IPERF3_METRICS_PREFIX" => Some("nettest".to_owned()),
            "IPERF3_PUSH_INTERVAL" => Some("2m".to_owned()),
            "IPERF3_PUSH_DELETE_ON_EXIT" => Some("yes".to_owned()),
            "IPERF3_METRICS_FILE" => Some("metrics.jsonl".to_owned()),
            "IPERF3_METRICS_FORMAT" => Some("prometheus".to_owned()),
            "IPERF3_METRICS_LABELS" => Some("site=ci,run=nightly".to_owned()),
            _ => None,
        })
        .unwrap();

        assert_eq!(app.push_url.unwrap().as_str(), "http://localhost:9091/");
        assert_eq!(app.push_timeout, Duration::from_millis(500));
        assert_eq!(app.push_retries, 3);
        assert_eq!(app.push_user_agent, "iperf3-rs/env");
        assert_eq!(app.metrics_prefix, "nettest");
        assert_eq!(app.push_interval, Some(Duration::from_secs(120)));
        assert!(app.push_delete_on_exit);
        assert_eq!(app.metrics_file, Some(PathBuf::from("metrics.jsonl")));
        assert_eq!(app.metrics_format, MetricsFileFormat::Prometheus);
        assert_eq!(
            app.metrics_labels,
            [
                ("site".to_owned(), "ci".to_owned()),
                ("run".to_owned(), "nightly".to_owned())
            ]
        );
        assert_eq!(iperf, ["iperf3-rs", "-s"]);
    }

    #[test]
    fn rejects_push_settings_without_push_url() {
        for option in [
            "--push.timeout=5s",
            "--push.retries=1",
            "--push.user-agent=iperf3-rs/test",
            "--push.interval=10s",
            "--push.delete-on-exit",
        ] {
            let args = vec!["iperf3-rs".to_owned(), option.to_owned()];

            let err = extract_app_options_with_env(args, |_| None).unwrap_err();
            assert!(
                err.to_string()
                    .contains("push settings require --push.url or IPERF3_PUSH_URL"),
                "{option} should require Pushgateway to be enabled"
            );
        }
    }

    #[test]
    fn parses_metrics_prefix_for_pushgateway_without_file_metrics() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.url=localhost:9091".to_owned(),
            "--metrics.prefix=nettest".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
        ];

        let (app, iperf) = extract_app_options_with_env(args, |_| None).unwrap();
        assert_eq!(app.push_url.unwrap().as_str(), "http://localhost:9091/");
        assert!(app.metrics_file.is_none());
        assert_eq!(app.metrics_prefix, "nettest");
        assert_eq!(iperf, ["iperf3-rs", "-c", "127.0.0.1"]);
    }

    #[test]
    fn metrics_prefix_requires_an_output_sink() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--metrics.prefix=nettest".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(err.to_string().contains("metric prefix requires"));
    }

    #[test]
    fn rejects_metrics_settings_without_metrics_file() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--metrics.format=prometheus".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("metrics settings require --metrics.file"),
            "{err:#}"
        );
    }

    #[test]
    fn metrics_labels_require_prometheus_file_output() {
        let missing_file = vec!["iperf3-rs".to_owned(), "--metrics.label=site=ci".to_owned()];
        let err = extract_app_options_with_env(missing_file, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--metrics.label requires --metrics.file")
        );

        let jsonl_file = vec![
            "iperf3-rs".to_owned(),
            "--metrics.file=metrics.jsonl".to_owned(),
            "--metrics.label=site=ci".to_owned(),
        ];
        let err = extract_app_options_with_env(jsonl_file, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--metrics.label requires --metrics.format prometheus")
        );
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
                "--metrics.prefix",
                "bad-prefix",
                "invalid --metrics.prefix metric prefix",
            ),
            (
                "--push.interval",
                "0",
                "--push.interval must be greater than zero",
            ),
            (
                "--push.interval",
                "1h",
                "invalid --push.interval duration: 1h",
            ),
        ] {
            let args = vec![
                "iperf3-rs".to_owned(),
                "--metrics.file".to_owned(),
                "metrics.prom".to_owned(),
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
    fn rejects_malformed_push_delete_on_exit_value() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.url".to_owned(),
            "localhost:9091".to_owned(),
            "--push.delete-on-exit=maybe".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--push.delete-on-exit must be one of"),
            "{err:#}"
        );
    }

    #[test]
    fn rejects_malformed_metrics_format() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--metrics.file=metrics.out".to_owned(),
            "--metrics.format=xml".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--metrics.format must be one of jsonl or prometheus"),
            "{err:#}"
        );
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
    fn parses_bool_options() {
        for value in ["1", "true", "yes", "on"] {
            assert!(parse_bool_option("--push.delete-on-exit", value).unwrap());
        }
        for value in ["0", "false", "no", "off"] {
            assert!(!parse_bool_option("--push.delete-on-exit", value).unwrap());
        }
        assert!(parse_bool_option("--push.delete-on-exit", "maybe").is_err());
    }

    #[test]
    fn parses_metrics_formats() {
        assert_eq!(
            parse_metrics_format("--metrics.format", "jsonl").unwrap(),
            MetricsFileFormat::Jsonl
        );
        assert_eq!(
            parse_metrics_format("--metrics.format", "prometheus").unwrap(),
            MetricsFileFormat::Prometheus
        );
        assert!(parse_metrics_format("--metrics.format", "xml").is_err());
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
                "IPERF3_PUSH_LABELS" => Some("not-a-label".to_owned()),
                "IPERF3_PUSH_TIMEOUT" => Some("not-a-duration".to_owned()),
                "IPERF3_PUSH_RETRIES" => Some("not-a-number".to_owned()),
                "IPERF3_PUSH_INTERVAL" => Some("not-a-duration".to_owned()),
                "IPERF3_PUSH_DELETE_ON_EXIT" => Some("not-a-bool".to_owned()),
                "IPERF3_METRICS_FORMAT" => Some("not-a-format".to_owned()),
                "IPERF3_METRICS_LABELS" => Some("not-a-label".to_owned()),
                _ => None,
            })
            .unwrap();

            assert!(app.show_help || app.show_version);
        }
    }
}
