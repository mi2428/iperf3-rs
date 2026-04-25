use std::env;

use anyhow::{Result, anyhow, bail};
use url::Url;

#[derive(Debug)]
pub struct AppOptions {
    pub push_gateway_url: Option<Url>,
    pub job: String,
    pub test: String,
    pub scenario: String,
    pub mirror_json: bool,
}

pub fn extract_app_options(args: Vec<String>) -> Result<(AppOptions, Vec<String>)> {
    extract_app_options_with_env(args, |key| env::var(key).ok())
}

fn extract_app_options_with_env(
    args: Vec<String>,
    mut get_env: impl FnMut(&str) -> Option<String>,
) -> Result<(AppOptions, Vec<String>)> {
    let mut pass_through = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    let program = iter.next().ok_or_else(|| anyhow!("missing argv[0]"))?;
    pass_through.push(program);

    let mut push_gateway = get_env("PUSH_GATEWAY_URL");
    let mut job = get_env("JOB_NAME").unwrap_or_else(|| "iperf3".to_owned());
    let mut test = get_env("TEST_NAME").unwrap_or_else(|| "testrun".to_owned());
    let mut scenario = get_env("SCENARIO_NAME").unwrap_or_else(|| "sample1".to_owned());
    let mut mirror_json = false;

    let rest: Vec<String> = iter.collect();
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if arg == "--" {
            pass_through.extend(rest[i..].iter().cloned());
            break;
        }

        if observes_json_output(arg) {
            mirror_json = true;
        }

        if let Some((key, value)) = split_long_value(arg) {
            match key {
                "--push-gateway" => push_gateway = Some(value.to_owned()),
                "--job" => job = value.to_owned(),
                "--test" => test = value.to_owned(),
                "--scenario" => scenario = value.to_owned(),
                _ => pass_through.push(arg.clone()),
            }
            i += 1;
            continue;
        }

        match arg.as_str() {
            "--push-gateway" => {
                push_gateway = Some(take_value(&rest, &mut i, "--push-gateway")?);
            }
            "--job" => {
                job = take_value(&rest, &mut i, "--job")?;
            }
            "--test" => {
                test = take_value(&rest, &mut i, "--test")?;
            }
            "--scenario" => {
                scenario = take_value(&rest, &mut i, "--scenario")?;
            }
            _ => {
                pass_through.push(arg.clone());
                i += 1;
            }
        }
    }

    let push_gateway_url = push_gateway.as_deref().map(parse_url).transpose()?;
    if push_gateway_url.is_some() && (job.is_empty() || test.is_empty() || scenario.is_empty()) {
        bail!("--job, --test, and --scenario must not be empty when --push-gateway is set");
    }

    Ok((
        AppOptions {
            push_gateway_url,
            job,
            test,
            scenario,
            mirror_json,
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
    let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    Url::parse(&with_scheme).map_err(|err| anyhow!("invalid --push-gateway URL: {err}"))
}

fn observes_json_output(arg: &str) -> bool {
    arg == "-J" || arg == "--json" || arg == "--json-stream" || arg == "--json-stream-full-output"
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
            "--push-gateway".to_owned(),
            "localhost:9091".to_owned(),
            "--job=net".to_owned(),
            "-t".to_owned(),
            "3".to_owned(),
        ];

        let (app, iperf) = extract_app_options(args).unwrap();
        assert_eq!(
            app.push_gateway_url.unwrap().as_str(),
            "http://localhost:9091/"
        );
        assert_eq!(app.job, "net");
        assert_eq!(iperf, ["iperf3-rs", "-c", "127.0.0.1", "-t", "3"]);
    }

    #[test]
    fn cli_values_override_environment_defaults() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "-s".to_owned(),
            "--push-gateway=http://cli.example:9091".to_owned(),
            "--job".to_owned(),
            "cli-job".to_owned(),
        ];

        let (app, iperf) = extract_app_options_with_env(args, |key| match key {
            "PUSH_GATEWAY_URL" => Some("http://env.example:9091".to_owned()),
            "JOB_NAME" => Some("env-job".to_owned()),
            "TEST_NAME" => Some("env-test".to_owned()),
            "SCENARIO_NAME" => Some("env-scenario".to_owned()),
            _ => None,
        })
        .unwrap();

        assert_eq!(
            app.push_gateway_url.unwrap().as_str(),
            "http://cli.example:9091/"
        );
        assert_eq!(app.job, "cli-job");
        assert_eq!(app.test, "env-test");
        assert_eq!(app.scenario, "env-scenario");
        assert_eq!(iperf, ["iperf3-rs", "-s"]);
    }

    #[test]
    fn preserves_arguments_after_double_dash() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
            "--".to_owned(),
            "--push-gateway".to_owned(),
            "ignored-by-wrapper".to_owned(),
        ];

        let (app, iperf) = extract_app_options_with_env(args, |_| None).unwrap();
        assert!(app.push_gateway_url.is_none());
        assert_eq!(
            iperf,
            [
                "iperf3-rs",
                "-c",
                "127.0.0.1",
                "--",
                "--push-gateway",
                "ignored-by-wrapper"
            ]
        );
    }

    #[test]
    fn rejects_missing_custom_option_value() {
        let args = vec!["iperf3-rs".to_owned(), "--push-gateway".to_owned()];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(err.to_string().contains("--push-gateway requires a value"));
    }

    #[test]
    fn rejects_empty_grouping_when_pushgateway_is_enabled() {
        let args = vec![
            "iperf3-rs".to_owned(),
            "--push-gateway".to_owned(),
            "localhost:9091".to_owned(),
            "--scenario=".to_owned(),
        ];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(
            err.to_string()
                .contains("--job, --test, and --scenario must not be empty")
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
}
