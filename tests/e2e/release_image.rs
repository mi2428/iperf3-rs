use super::helpers::*;

// This smoke test protects the Docker image shape used by release publishing.
//
// The Compose E2E test above runs in the `e2e` target,
// which intentionally includes test tools such as curl and upstream iperf3.
// The image published to GHCR is the `release` target instead: a scratch image
// that contains only the iperf3-rs binary and the minimal writable filesystem
// libiperf needs at runtime. Building that target, running `--version`, and
// completing a release-image-to-release-image iperf run verifies that the final
// image can start without a shell or dynamic loader and still create real test
// streams. Protocol interoperability with upstream iperf3 and Pushgateway
// behavior remain covered by the Compose test above.
#[test]
#[ignore = "requires Docker"]
fn release_image_smoke() {
    let image = ReleaseImage::build();
    let output = DockerContainer::output(&image.docker, &image.tag, None, &["--version"]);

    assert_command_success(&format!("release image {} --version", image.tag), &output);

    let version_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        version_output.to_ascii_lowercase().contains("iperf"),
        "release image --version output should identify iperf\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let network = DockerNetwork::create(&image.docker);
    let server = DockerContainer::run_detached(&image.docker, &image.tag, &network.name, &["-s"]);
    let release_to_release =
        retry_json_client("release image client to release image server", || {
            DockerContainer::output(
                &image.docker,
                &image.tag,
                Some(&network.name),
                &["-c", &server.name, "-t", "1", "-i", "1", "-J"],
            )
        });
    assert_iperf_summary_has_traffic(&release_to_release);
}
