use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use url::Url;

use crate::metrics::checked_deadline;
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
    extract_app_options_with_env(args, |key| match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{key} must be valid UTF-8"),
    })
}

fn extract_app_options_with_env(
    args: Vec<String>,
    mut get_env: impl FnMut(&str) -> Result<Option<String>>,
) -> Result<(AppOptions, Vec<String>)> {
    // iperf3-rs options are consumed here so libiperf receives an argv that still
    // looks like the upstream iperf3 CLI.
    let mut pass_through = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    let program = iter.next().ok_or_else(|| anyhow!("missing argv[0]"))?;
    pass_through.push(program);

    let rest: Vec<String> = iter.collect();
    let (spans, show_help, show_version) = argument_boundaries(&rest)?;
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

    let mut push_url = None;
    let mut push_job = None;
    let mut push_labels = get_env("IPERF3_PUSH_LABELS")?
        .map(|raw| parse_env_labels("IPERF3_PUSH_LABELS", &raw, true))
        .transpose()?
        .unwrap_or_default();
    let mut push_timeout = None;
    let mut push_retries = None;
    let mut push_user_agent = None;
    let mut metrics_prefix = None;
    let mut push_interval = None;
    let mut push_delete_on_exit = None;
    let mut metrics_file = None;
    let mut metrics_format = None;
    let mut metrics_labels = get_env("IPERF3_METRICS_LABELS")?
        .map(|raw| parse_env_labels("IPERF3_METRICS_LABELS", &raw, false))
        .transpose()?
        .unwrap_or_default();
    let mut saw_push_job = false;
    let mut saw_push_label = !push_labels.is_empty();
    let mut saw_push_setting = false;
    let mut saw_metrics_setting = false;
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

        if spans[i] != 0 {
            pass_through.extend(rest[i..i + spans[i]].iter().cloned());
            i += spans[i];
            continue;
        }

        if let Some((key, value)) = split_long_value(arg) {
            match key {
                "--push.url" => push_url = Some(value.to_owned()),
                "--push.job" => {
                    push_job = Some(value.to_owned());
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
                    push_timeout = Some(parse_duration_option("--push.timeout", value)?);
                    saw_push_setting = true;
                }
                "--push.retries" => {
                    push_retries = Some(parse_retries("--push.retries", value)?);
                    saw_push_setting = true;
                }
                "--push.user-agent" => {
                    push_user_agent = Some(parse_user_agent("--push.user-agent", value)?);
                    saw_push_setting = true;
                }
                "--metrics.prefix" => {
                    metrics_prefix = Some(parse_metric_prefix("--metrics.prefix", value)?);
                    saw_metric_prefix = true;
                }
                "--push.interval" => {
                    push_interval = Some(parse_duration_option("--push.interval", value)?);
                    saw_push_setting = true;
                }
                "--push.delete-on-exit" => {
                    push_delete_on_exit = Some(parse_bool_option("--push.delete-on-exit", value)?);
                    saw_push_setting = true;
                }
                "--metrics.file" => {
                    metrics_file = Some(PathBuf::from(value));
                }
                "--metrics.format" => {
                    metrics_format = Some(parse_metrics_format("--metrics.format", value)?);
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
                push_job = Some(take_value(&rest, &mut i, "--push.job")?);
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
                push_timeout = Some(parse_duration_option(
                    "--push.timeout",
                    &take_value(&rest, &mut i, "--push.timeout")?,
                )?);
                saw_push_setting = true;
            }
            "--push.retries" => {
                push_retries = Some(parse_retries(
                    "--push.retries",
                    &take_value(&rest, &mut i, "--push.retries")?,
                )?);
                saw_push_setting = true;
            }
            "--push.user-agent" => {
                push_user_agent = Some(parse_user_agent(
                    "--push.user-agent",
                    &take_value(&rest, &mut i, "--push.user-agent")?,
                )?);
                saw_push_setting = true;
            }
            "--metrics.prefix" => {
                metrics_prefix = Some(parse_metric_prefix(
                    "--metrics.prefix",
                    &take_value(&rest, &mut i, "--metrics.prefix")?,
                )?);
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
                push_delete_on_exit = Some(true);
                saw_push_setting = true;
                i += 1;
            }
            "--metrics.file" => {
                metrics_file = Some(PathBuf::from(take_value(&rest, &mut i, "--metrics.file")?));
            }
            "--metrics.format" => {
                metrics_format = Some(parse_metrics_format(
                    "--metrics.format",
                    &take_value(&rest, &mut i, "--metrics.format")?,
                )?);
                saw_metrics_setting = true;
            }
            _ => {
                pass_through.push(arg.clone());
                i += 1;
            }
        }
    }

    // Only defaults that were not replaced by CLI values are effective inputs.
    let push_timeout = env_default(
        push_timeout,
        "IPERF3_PUSH_TIMEOUT",
        &mut get_env,
        parse_duration_option,
    )?
    .unwrap_or_else(PushGatewayConfig::default_timeout);
    let push_retries = env_default(
        push_retries,
        "IPERF3_PUSH_RETRIES",
        &mut get_env,
        parse_retries,
    )?
    .unwrap_or(PushGatewayConfig::DEFAULT_RETRIES);
    let push_user_agent = env_default(
        push_user_agent,
        "IPERF3_PUSH_USER_AGENT",
        &mut get_env,
        parse_user_agent,
    )?
    .unwrap_or_else(PushGatewayConfig::default_user_agent);
    let metrics_prefix = env_default(
        metrics_prefix,
        "IPERF3_METRICS_PREFIX",
        &mut get_env,
        parse_metric_prefix,
    )?
    .unwrap_or_else(|| PushGatewayConfig::DEFAULT_METRIC_PREFIX.to_owned());
    let push_interval = env_default(
        push_interval,
        "IPERF3_PUSH_INTERVAL",
        &mut get_env,
        parse_duration_option,
    )?;
    let push_delete_on_exit = env_default(
        push_delete_on_exit,
        "IPERF3_PUSH_DELETE_ON_EXIT",
        &mut get_env,
        parse_bool_option,
    )?
    .unwrap_or(false);
    let metrics_format = env_default(
        metrics_format,
        "IPERF3_METRICS_FORMAT",
        &mut get_env,
        parse_metrics_format,
    )?;
    saw_metrics_setting |= metrics_format.is_some();
    let metrics_format = metrics_format.unwrap_or(MetricsFileFormat::Jsonl);
    let push_url = env_default(push_url, "IPERF3_PUSH_URL", &mut get_env, |_, raw| {
        Ok(raw.to_owned())
    })?
    .as_deref()
    .map(parse_url)
    .transpose()?;
    let push_job = env_default(push_job, "IPERF3_PUSH_JOB", &mut get_env, |_, raw| {
        Ok(raw.to_owned())
    })?
    .unwrap_or_else(|| PushGatewayConfig::DEFAULT_JOB.to_owned());
    let metrics_file = env_default(
        metrics_file,
        "IPERF3_METRICS_FILE",
        &mut get_env,
        |_, raw| Ok(PathBuf::from(raw)),
    )?;
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

fn env_default<T>(
    cli: Option<T>,
    key: &str,
    get_env: &mut impl FnMut(&str) -> Result<Option<String>>,
    parse: impl FnOnce(&str, &str) -> Result<T>,
) -> Result<Option<T>> {
    match cli {
        Some(value) => Ok(Some(value)),
        None => get_env(key)?.map(|raw| parse(key, &raw)).transpose(),
    }
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
    checked_deadline(duration).map_err(|err| anyhow!("invalid {option}: {err}"))?;
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

fn argument_boundaries(args: &[String]) -> Result<(Vec<usize>, bool, bool)> {
    let _guard = crate::command::run_lock()
        .lock()
        .map_err(|_| anyhow!("libiperf run lock is poisoned"))?;
    let mut spans = vec![1; args.len()];
    let mut index = 0;
    while index < args.len() {
        let word = &args[index];
        if word == "--" {
            break;
        }
        let (key, inline) =
            split_long_value(word).map_or((word.as_str(), false), |(key, _)| (key, true));
        let wrapper_value = match key {
            "--push.url" | "--push.job" | "--push.label" | "--push.timeout" | "--push.retries"
            | "--push.user-agent" | "--push.interval" | "--metrics.file" | "--metrics.format"
            | "--metrics.label" | "--metrics.prefix" => Some(true),
            "--push.delete-on-exit" => Some(false),
            _ => None,
        };
        if let Some(requires_value) = wrapper_value {
            spans[index] = 0;
            index += if requires_value && !inline && index + 1 < args.len() {
                2
            } else {
                1
            };
        } else {
            let (span, info) =
                crate::iperf::arg_boundary(word, args.get(index + 1).map(String::as_str))?;
            spans[index] = span;
            if info > 0 {
                return Ok((spans, info == 1, info == 2));
            }
            if info < 0 {
                // Keep the remaining argv untouched for upstream's error path.
                break;
            }
            index += span;
        }
    }
    Ok((spans, false, false))
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
        let expected = name == b"job" || name.starts_with(b"__");

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

    fn extract_app_options_with_env(
        args: Vec<String>,
        mut get_env: impl FnMut(&str) -> Option<String>,
    ) -> Result<(AppOptions, Vec<String>)> {
        super::extract_app_options_with_env(args, |key| Ok(get_env(key)))
    }

    #[test]
    fn upstream_operands_are_never_wrapper_or_information_options() {
        for option in ["--extra-data", "--extr", "--title", "-T", "-F"] {
            for value in [
                "--help",
                "--version",
                "--push.timeout=bad",
                "--metrics.file=x",
                "--",
            ] {
                let args: Vec<_> = ["iperf3-rs", "-c", "127.0.0.1", option, value]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                let (app, forwarded) =
                    extract_app_options_with_env(args.clone(), |_| None).unwrap();
                assert_eq!(forwarded, args);
                assert!(!app.show_help && !app.show_version);
                assert!(app.push_url.is_none() && app.metrics_file.is_none());
            }
        }
        for word in [
            "--extra-data=--help",
            "-T--help",
            "-VT--help",
            "--debug=--help",
            "--timestamps=--help",
        ] {
            let args = vec!["iperf3-rs".to_owned(), word.to_owned()];
            let (app, forwarded) = extract_app_options_with_env(args.clone(), |_| None).unwrap();
            assert_eq!(forwarded, args);
            assert!(!app.show_help && !app.show_version);
        }
    }

    #[test]
    fn wrapper_operands_do_not_request_help() {
        for option in ["--push.job", "--push.user-agent", "--metrics.file"] {
            let args = [
                "iperf3-rs",
                "--push.url=localhost:9091",
                "-s",
                option,
                "--help",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            let (app, forwarded) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(!app.show_help && !app.show_version);
            assert_eq!(forwarded, ["iperf3-rs", "-s"]);
        }
    }

    #[test]
    fn native_information_forms_skip_defaults_but_unknown_options_do_not() {
        for (flag, help) in [
            ("-hV", true),
            ("--hel", true),
            ("-Vv", false),
            ("--version", false),
        ] {
            let args = vec!["iperf3-rs".to_owned(), flag.to_owned()];
            let (app, _) =
                super::extract_app_options_with_env(args, |_| bail!("env must not be read"))
                    .unwrap();
            assert_eq!(app.show_help, help);
            assert_eq!(app.show_version, !help);
        }
        let args = vec![
            "iperf3-rs".to_owned(),
            "--unknown-option".to_owned(),
            "--help".to_owned(),
        ];
        let (app, forwarded) = extract_app_options_with_env(args.clone(), |_| None).unwrap();
        assert!(!app.show_help && !app.show_version);
        assert_eq!(forwarded, args);
    }

    #[test]
    fn native_optional_arguments_leave_the_next_wrapper_word_unconsumed() {
        for word in ["--timestamps", "--debug", "-d"] {
            let args = ["iperf3-rs", word, "--push.url=localhost:9091", "-s"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            let (app, forwarded) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(app.push_url.is_some());
            assert_eq!(forwarded, ["iperf3-rs", word, "-s"]);
        }
    }

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
    fn scalar_cli_values_replace_invalid_environment_defaults() {
        let values = [
            ("--push.timeout", "2s"),
            ("--push.interval", "3s"),
            ("--push.retries", "2"),
            ("--push.user-agent", "cli-agent"),
            ("--push.delete-on-exit", "true"),
            ("--metrics.prefix", "cli_prefix"),
            ("--metrics.format", "prometheus"),
        ];
        for inline in [true, false] {
            let mut args = vec![
                "iperf3-rs".to_owned(),
                "--push.url=localhost:9091".to_owned(),
                "--metrics.file=metrics.prom".to_owned(),
                "-s".to_owned(),
            ];
            for (option, value) in values {
                if inline {
                    args.push(format!("{option}={value}"));
                } else {
                    args.push(option.to_owned());
                    if option != "--push.delete-on-exit" {
                        args.push(value.to_owned());
                    }
                }
            }
            let (app, iperf) = extract_app_options_with_env(args, |key| match key {
                "IPERF3_PUSH_TIMEOUT"
                | "IPERF3_PUSH_INTERVAL"
                | "IPERF3_PUSH_RETRIES"
                | "IPERF3_PUSH_DELETE_ON_EXIT"
                | "IPERF3_METRICS_PREFIX"
                | "IPERF3_METRICS_FORMAT" => Some("invalid!".to_owned()),
                "IPERF3_PUSH_USER_AGENT" => Some("invalid\nagent".to_owned()),
                _ => None,
            })
            .unwrap();
            assert_eq!(app.push_timeout, Duration::from_secs(2));
            assert_eq!(app.push_interval, Some(Duration::from_secs(3)));
            assert_eq!(app.push_retries, 2);
            assert_eq!(app.push_user_agent, "cli-agent");
            assert!(app.push_delete_on_exit);
            assert_eq!(app.metrics_prefix, "cli_prefix");
            assert_eq!(app.metrics_format, MetricsFileFormat::Prometheus);
            assert_eq!(iperf, ["iperf3-rs", "-s"]);
        }
    }

    #[test]
    fn invalid_effective_scalar_values_never_fall_back() {
        for (key, option, valid, invalid) in [
            ("IPERF3_PUSH_TIMEOUT", "--push.timeout", "2s", "bad"),
            ("IPERF3_PUSH_INTERVAL", "--push.interval", "3s", "bad"),
            ("IPERF3_PUSH_RETRIES", "--push.retries", "2", "bad"),
            (
                "IPERF3_PUSH_USER_AGENT",
                "--push.user-agent",
                "agent",
                "bad\nagent",
            ),
            (
                "IPERF3_PUSH_DELETE_ON_EXIT",
                "--push.delete-on-exit",
                "true",
                "bad",
            ),
            (
                "IPERF3_METRICS_PREFIX",
                "--metrics.prefix",
                "prefix",
                "bad-prefix",
            ),
            ("IPERF3_METRICS_FORMAT", "--metrics.format", "jsonl", "bad"),
        ] {
            let args = vec![
                "iperf3-rs".to_owned(),
                "--push.url=localhost:9091".to_owned(),
                "--metrics.file=metrics.jsonl".to_owned(),
            ];
            let err = extract_app_options_with_env(args.clone(), |name| {
                (name == key).then(|| invalid.to_owned())
            })
            .unwrap_err();
            assert!(err.to_string().contains(key), "{err}");

            for inline in [true, false] {
                let mut args = args.clone();
                if inline || option == "--push.delete-on-exit" {
                    args.push(format!("{option}={invalid}"));
                } else {
                    args.extend([option.to_owned(), invalid.to_owned()]);
                }
                let err = extract_app_options_with_env(args, |name| {
                    (name == key).then(|| valid.to_owned())
                })
                .unwrap_err();
                assert!(err.to_string().contains(option), "{err}");
            }
        }
    }

    #[test]
    fn repeated_scalars_keep_last_valid_value_and_reject_invalid_occurrences() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push.url=localhost:9091".to_owned(),
            "--push.timeout=1s".to_owned(),
            "--push.timeout=2s".to_owned(),
        ];
        let (app, _) = extract_app_options_with_env(args.clone(), |_| None).unwrap();
        assert_eq!(app.push_timeout, Duration::from_secs(2));
        let mut invalid = args;
        invalid[2] = "--push.timeout=bad".to_owned();
        assert!(extract_app_options_with_env(invalid, |_| None).is_err());
    }

    #[test]
    fn unicode_values_and_additive_labels_are_preserved() {
        let (app, _) = extract_app_options_with_env(
            [
                "iperf3-rs",
                "--push.url=localhost:9091",
                "--push.job=測定",
                "--push.label=site=東京",
                "--metrics.file=測定.prom",
                "--metrics.format=prometheus",
                "--metrics.label=site=東京",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            |key| match key {
                "IPERF3_PUSH_LABELS" | "IPERF3_METRICS_LABELS" => Some("region=日本".to_owned()),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(app.push_job, "測定");
        assert_eq!(app.metrics_file, Some(PathBuf::from("測定.prom")));
        let expected = vec![
            ("region".to_owned(), "日本".to_owned()),
            ("site".to_owned(), "東京".to_owned()),
        ];
        assert_eq!(app.push_labels, expected);
        assert_eq!(app.metrics_labels, expected);
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
