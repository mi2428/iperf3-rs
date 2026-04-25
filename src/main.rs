mod args;
mod help;
mod iperf;
mod metrics;
mod pushgateway;
mod version;

use std::env;
use std::process::ExitCode;

use anyhow::{Context, Result};
use args::extract_app_options;
use iperf::{IperfTest, Role};
use metrics::JsonMetricsReporter;
use pushgateway::{PushGateway, PushGatewayConfig};

const EXIT_OPTION_ERROR: u8 = 1;
const EXIT_IPERF_ERROR: u8 = 2;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::from(EXIT_IPERF_ERROR)
        }
    }
}

fn run() -> Result<()> {
    let raw_args: Vec<String> = env::args().collect();
    // Split wrapper-only push options before handing argv to libiperf's own
    // parser, preserving upstream iperf3 option compatibility.
    let (app, iperf_args) = extract_app_options(raw_args).map_err(|err| {
        eprintln!("{err:#}");
        std::process::exit(EXIT_OPTION_ERROR.into());
    })?;
    if app.show_help {
        print!("{}", help::render_full_help(&iperf::usage_long()?));
        return Ok(());
    }
    if app.show_version {
        let libiperf_version = iperf::libiperf_version();
        print!("{}", version::render(&version::current(&libiperf_version)));
        return Ok(());
    }

    let mut test = IperfTest::new().context("failed to create iperf test")?;
    test.parse_arguments(&iperf_args)?;

    let reporter = if let Some(push_url) = app.push_url {
        // Role is known only after libiperf parses argv, so the automatic
        // `iperf_mode` grouping label is attached here.
        let mode = match test.role() {
            Role::Client => "client",
            Role::Server => "server",
            Role::Unknown(_) => "unknown",
        };
        let mut labels = app.push_labels;
        labels.push(("iperf_mode".to_owned(), mode.to_owned()));
        let sink = PushGateway::new(PushGatewayConfig {
            endpoint: push_url,
            job: app.push_job,
            labels,
            timeout: app.push_timeout,
            retries: app.push_retries,
            user_agent: app.push_user_agent,
            metric_prefix: app.push_metric_prefix,
        })?;
        Some(JsonMetricsReporter::attach(
            &mut test,
            sink,
            app.mirror_json,
        )?)
    } else {
        None
    };

    test.run()?;

    // Dropping the reporter unregisters the C callback and drains the worker
    // thread after libiperf has stopped producing JSON events.
    drop(reporter);
    Ok(())
}
