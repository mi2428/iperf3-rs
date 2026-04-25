#define _GNU_SOURCE

#include "iperf_config.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>

#include "iperf.h"
#include "iperf_api.h"
#include "iperf3rs_shim.h"

static iperf3rs_metrics_callback interval_metrics_callback = NULL;

static void iperf3rs_reporter_callback(struct iperf_test *test);
static void iperf3rs_emit_interval_metrics(struct iperf_test *test);

void
iperf3rs_enable_interval_metrics(struct iperf_test *test, iperf3rs_metrics_callback callback)
{
    interval_metrics_callback = callback;
    test->reporter_callback = iperf3rs_reporter_callback;
}

static void
iperf3rs_reporter_callback(struct iperf_test *test)
{
    iperf_reporter_callback(test);
    iperf3rs_emit_interval_metrics(test);
}

static void
iperf3rs_emit_interval_metrics(struct iperf_test *test)
{
    struct iperf_stream *stream = NULL;
    struct iperf_interval_results *interval = NULL;
    double bytes = 0.0;
    double bandwidth_bits_per_second = 0.0;
    double packets = 0.0;
    double error_packets = 0.0;
    double jitter_seconds = 0.0;
    double tcp_retransmits = 0.0;
    double interval_duration = 0.0;
    int matched_streams = 0;
    int interval_ok = 0;
    int stream_must_be_sender;

    if (interval_metrics_callback == NULL) {
        return;
    }

    if (test->mode == BIDIRECTIONAL) {
        stream_must_be_sender = test->role == 'c';
    } else {
        stream_must_be_sender = test->mode * test->mode;
    }

    SLIST_FOREACH(stream, &test->streams, streams) {
        if (stream->sender != stream_must_be_sender) {
            continue;
        }

        interval = TAILQ_LAST(&stream->result->interval_results, irlisthead);
        if (interval == NULL) {
            continue;
        }

        if (interval->interval_duration >= test->stats_interval * 0.10 ||
            interval->bytes_transferred > 0) {
            interval_ok = 1;
        }

        bytes += (double)interval->bytes_transferred;
        packets += (double)interval->interval_packet_count;
        error_packets += (double)interval->interval_cnt_error;
        jitter_seconds += interval->jitter;
        if (test->protocol->id == Ptcp && test->sender_has_retransmits == 1 && stream_must_be_sender) {
            tcp_retransmits += (double)interval->interval_retrans;
        }
        if (matched_streams == 0) {
            interval_duration = interval->interval_duration;
        }
        matched_streams += 1;
    }

    if (!interval_ok || matched_streams == 0) {
        return;
    }

    if (interval_duration > 0.0) {
        bandwidth_bits_per_second = bytes * 8.0 / interval_duration;
    }
    jitter_seconds /= matched_streams;

    interval_metrics_callback(
        test,
        bytes,
        bandwidth_bits_per_second,
        packets,
        error_packets,
        jitter_seconds,
        tcp_retransmits);
}

int
iperf3rs_run_server_once(struct iperf_test *test)
{
    int rc = iperf_run_server(test);
    test->server_last_run_rc = rc;
    return rc;
}

int
iperf3rs_current_errno(void)
{
    return i_errno;
}

int
iperf3rs_is_auth_test_error(void)
{
    return i_errno == IEAUTHTEST;
}

const char *
iperf3rs_current_error(void)
{
    return iperf_strerror(i_errno);
}

void
iperf3rs_ignore_sigpipe(void)
{
#ifdef SIGPIPE
    signal(SIGPIPE, SIG_IGN);
#endif
}

char *
iperf3rs_usage_long(void)
{
    char *buffer = NULL;
    size_t length = 0;
    FILE *stream = open_memstream(&buffer, &length);
    if (stream == NULL) {
        return NULL;
    }

    usage_long(stream);
    if (fclose(stream) != 0) {
        free(buffer);
        return NULL;
    }

    return buffer;
}

void
iperf3rs_free_string(char *value)
{
    free(value);
}
