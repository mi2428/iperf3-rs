//! Hidden entry point used by the binary target.

use std::env;
use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::args::extract_app_options;
use crate::help;
use crate::iperf::IperfTest;
use crate::metrics::{IntervalMetricsReporter, MetricsSinks};
use crate::metrics_file::MetricsFileSink;
use crate::pushgateway::{PushGateway, PushGatewayConfig};
use crate::version;

const EXIT_OPTION_ERROR: u8 = 1;
const EXIT_IPERF_ERROR: u8 = 2;

/// Run the iperf3-rs CLI and return its process exit code.
pub fn main() -> ExitCode {
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
    // Split wrapper-only metrics options before handing argv to libiperf's own
    // parser, preserving upstream iperf3 option compatibility.
    let (app, iperf_args) = extract_app_options(raw_args).map_err(|err| {
        eprintln!("{err:#}");
        std::process::exit(EXIT_OPTION_ERROR.into());
    })?;
    if app.show_help {
        print!("{}", help::render_full_help(&crate::iperf::usage_long()?));
        return Ok(());
    }
    if app.show_version {
        let libiperf_version = crate::iperf::libiperf_version();
        print!("{}", version::render(&version::current(&libiperf_version)));
        return Ok(());
    }

    let mut test = IperfTest::new().context("failed to create iperf test")?;
    test.parse_arguments(&iperf_args)?;

    let mut sinks = MetricsSinks::new();
    if let Some(push_url) = app.push_url {
        let config = PushGatewayConfig::new(push_url)
            .job(app.push_job)
            .labels(app.push_labels)
            .timeout(app.push_timeout)
            .retries(app.push_retries)
            .user_agent(app.push_user_agent)
            .metric_prefix(app.metrics_prefix.clone())
            .delete_on_finish(app.push_delete_on_exit);
        let sink = PushGateway::new(config)?;
        sinks.pushgateway(sink, app.push_interval);
    }
    if let Some(metrics_file) = app.metrics_file {
        sinks.file(MetricsFileSink::with_prefix_and_labels(
            metrics_file,
            app.metrics_format,
            app.metrics_prefix,
            app.metrics_labels,
        )?);
    }

    let reporter = if sinks.is_empty() {
        None
    } else {
        Some(IntervalMetricsReporter::attach_sinks(&mut test, sinks)?)
    };

    let result = test.run();

    // Finishing the reporter unregisters the C callback, drains the worker
    // thread, and surfaces required file sink errors after libiperf stops.
    let reporter_result = reporter.map(IntervalMetricsReporter::finish).transpose();

    result?;
    reporter_result?;
    Ok(())
}
