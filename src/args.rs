use std::env;

use anyhow::{Result, anyhow, bail};
use url::Url;

#[derive(Debug)]
pub struct AppOptions {
    pub push_url: Option<Url>,
    pub push_job: String,
    pub push_labels: Vec<(String, String)>,
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

    let mut push_url = get_env("PUSH_URL");
    let mut push_job = get_env("PUSH_JOB").unwrap_or_else(|| "iperf3".to_owned());
    let mut push_labels = get_env("PUSH_LABELS")
        .map(|raw| parse_env_labels(&raw))
        .transpose()?
        .unwrap_or_default();
    let mut saw_push_job = false;
    let mut saw_push_label = !push_labels.is_empty();
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
                "--push.url" => push_url = Some(value.to_owned()),
                "--push.job" => {
                    push_job = value.to_owned();
                    saw_push_job = true;
                }
                "--push.label" => {
                    push_labels.push(parse_label(value)?);
                    saw_push_label = true;
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
    if push_url.is_some() && push_job.is_empty() {
        bail!("--push.job must not be empty when --push.url is set");
    }
    reject_duplicate_labels(&push_labels)?;

    Ok((
        AppOptions {
            push_url,
            push_job,
            push_labels,
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
    Url::parse(&with_scheme).map_err(|err| anyhow!("invalid --push.url URL: {err}"))
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
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
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
        let args = vec!["iperf3-rs".to_owned(), "--push.url".to_owned()];

        let err = extract_app_options_with_env(args, |_| None).unwrap_err();
        assert!(err.to_string().contains("--push.url requires a value"));
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
    fn notices_user_requested_json_output() {
        for flag in ["-J", "--json", "--json-stream", "--json-stream-full-output"] {
            let args = vec!["iperf3-rs".to_owned(), flag.to_owned()];
            let (app, _) = extract_app_options_with_env(args, |_| None).unwrap();
            assert!(app.mirror_json, "{flag} should mirror JSON output");
        }
    }
}
