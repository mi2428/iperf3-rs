mod args;
mod iperf;
mod metrics;
mod pushgateway;

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
    let (app, iperf_args) = extract_app_options(raw_args).map_err(|err| {
        eprintln!("{err:#}");
        std::process::exit(EXIT_OPTION_ERROR.into());
    })?;

    let mut test = IperfTest::new().context("failed to create iperf test")?;
    test.parse_arguments(&iperf_args)?;

    let reporter = if let Some(push_url) = app.push_url {
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

    drop(reporter);
    Ok(())
}
