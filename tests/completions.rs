use std::{io::ErrorKind, path::Path, process::Command};

const ZSH: &str = include_str!("../completions/_iperf3-rs");

#[test]
fn zsh_optional_arguments_require_an_attached_equals_value() {
    // _arguments `=-` requires an attached value; `::` makes it optional.
    for (option, value) in [("timestamps", "format"), ("debug", "level")] {
        assert!(ZSH.lines().any(|line| {
            line.trim_start().starts_with(&format!("'--{option}=-["))
                && line.contains(&format!("]::{value}:"))
        }));
    }
    assert!(ZSH.contains("'-d[emit debugging output]'"));
    assert!(!ZSH.contains("{-d,--debug}"));
}

#[test]
fn fish_debug_levels_are_only_suggested_as_attached_values() {
    let completion = Path::new(env!("CARGO_MANIFEST_DIR")).join("completions/iperf3-rs.fish");
    for (line, expected) in [
        (
            "iperf3-rs --debug=",
            vec!["--debug=1", "--debug=2", "--debug=3", "--debug=4"],
        ),
        ("iperf3-rs --debug ", vec![]),
        ("iperf3-rs -d ", vec![]),
        ("iperf3-rs --debug --", vec!["--server"]),
        ("iperf3-rs --timestamps --", vec!["--server"]),
    ] {
        let result = Command::new("fish")
            .args([
                "--no-config",
                "-c",
                "source $argv[1]; complete -C $argv[2]",
                "--",
            ])
            .arg(&completion)
            .arg(line)
            .output();
        let output = match result {
            Ok(output) => output,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                // Like Makefile's completion checks, fish is optional locally.
                eprintln!("Skipping fish semantic check; fish not found");
                return;
            }
            Err(err) => panic!("run fish completion: {err}"),
        };
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let candidates: Vec<_> = stdout
            .lines()
            .map(|line| line.split('\t').next().unwrap())
            .collect();
        for candidate in expected {
            assert!(candidates.contains(&candidate), "{line}");
        }
        assert!(
            !stdout.lines().any(|candidate| {
                candidate
                    .split_once('\t')
                    .is_some_and(|(value, description)| {
                        ["1", "2", "3", "4"].contains(&value)
                            && description == "Emit debugging output"
                    })
            }),
            "{line}"
        );
    }
}

#[test]
fn wrapper_options_remain_in_all_shells_and_help() {
    let sources = [
        ZSH,
        include_str!("../completions/iperf3-rs.bash"),
        include_str!("../completions/iperf3-rs.fish"),
        include_str!("../src/help.rs"),
    ];
    for option in [
        "push.url",
        "push.delete-on-exit",
        "push.interval",
        "push.job",
        "push.label",
        "push.retries",
        "push.timeout",
        "push.user-agent",
        "metrics.file",
        "metrics.format",
        "metrics.label",
        "metrics.prefix",
    ] {
        assert!(
            sources.iter().all(|source| source.contains(option)),
            "{option}"
        );
    }
}
