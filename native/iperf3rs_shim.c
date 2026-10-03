#define _GNU_SOURCE

#include "iperf_config.h"

#include <errno.h>
#include <getopt.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "iperf.h"
#include "iperf_api.h"
#include "iperf3rs_shim.h"
#include "iperf3rs_options.h"
#include "iperf3rs_cli.h"

static iperf3rs_metrics_callback interval_metrics_callback = NULL;
/* Like the callback, this snapshot belongs to the serialized native run. */
static double interval_reorder_events = 0.0;
static int interval_reorder_available = 0;

static void iperf3rs_stats_callback(struct iperf_test *test);
static void iperf3rs_reporter_callback(struct iperf_test *test);
static void iperf3rs_emit_interval_metrics(struct iperf_test *test);
static int iperf3rs_add_nonnegative(double *sum, long value);

static void
iperf3rs_reset_getopt(void)
{
#if defined(__APPLE__) || defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) || defined(__DragonFly__)
    optreset = 1;
    optind = 1;
#else
    optind = 0;
#endif
    optarg = NULL;
}

void
iperf3rs_clear_error_state(void)
{
    i_errno = 0;
    errarg = NULL;
}

int
iperf3rs_parse_arguments(struct iperf_test *test, int argc, char **argv)
{
    int rc;
    iperf3rs_reset_getopt();
    iperf3rs_clear_error_state();
    rc = iperf_parse_arguments(test, argc, argv);
    iperf3rs_reset_getopt();
    return rc;
}

/* Classify a single upstream option word without applying its semantics. */
int
iperf3rs_arg_boundary(char *word, char *next, int *info)
{
    char program[] = "iperf3-rs";
    char *argv[] = { program, word, next, NULL };
    int argc = next == NULL ? 2 : 3;
    int flag;
    int consumed = 1;
    int previous_opterr = opterr;
    *info = 0;
    if (word[0] != '-' || word[1] == '\0') {
        return consumed;
    }
    opterr = 0;
    iperf3rs_reset_getopt();
    while ((flag = getopt_long(argc, argv, iperf3rs_shortopts, iperf3rs_longopts, NULL)) != -1) {
        if (optind > 2) {
            consumed = 2;
        }
        if (flag == '?' || flag == ':') {
            *info = -1; /* preserve the upstream unknown/missing option path */
            break;
        }
        if (flag == 'h' || flag == 'v') {
            *info = flag == 'h' ? 1 : 2;
            break;
        }
        if (optind >= 2) {
            break; /* Do not parse the following operand as another option. */
        }
    }
    iperf3rs_reset_getopt();
    opterr = previous_opterr;
    return consumed;
}

void
iperf3rs_enable_interval_metrics(struct iperf_test *test, iperf3rs_metrics_callback callback)
{
    interval_metrics_callback = callback;
    interval_reorder_events = 0.0;
    interval_reorder_available = 0;
    test->stats_callback = iperf3rs_stats_callback;
    test->reporter_callback = iperf3rs_reporter_callback;
}

long
iperf3rs_reorder_delta(long current, long previous)
{
    if (current < 0 || previous < 0) {
        return -1;
    }
    /* A decreasing counter starts a new epoch, rather than a negative delta. */
    return current < previous ? current : current - previous;
}

static int
iperf3rs_stream_must_be_sender(struct iperf_test *test)
{
    return test->mode == BIDIRECTIONAL ? test->role == 'c' : test->mode * test->mode;
}

static void
iperf3rs_stats_callback(struct iperf_test *test)
{
    struct iperf_stream *stream;
    struct iperf_interval_results *interval;
    size_t count = 0, index = 0;
    interval_reorder_events = 0.0;
    interval_reorder_available = 0;
    if (test->protocol->id != Ptcp || test->sender_has_retransmits != 1 ||
        !iperf3rs_stream_must_be_sender(test)) {
        iperf_stats_callback(test);
        return;
    }
    SLIST_FOREACH(stream, &test->streams, streams) {
        if (stream->sender) {
            count++;
        }
    }
    if (count == 0) {
        iperf_stats_callback(test);
        return;
    }
    /* Upstream replaces the last result during stats collection. Capture its
     * per-stream cumulative value first; native stream counts are bounded by
     * MAX_STREAMS (twice that in bidirectional mode). No cross-session history. */
    long previous[count];
    SLIST_FOREACH(stream, &test->streams, streams) {
        if (stream->sender) {
            interval = TAILQ_LAST(&stream->result->interval_results, irlisthead);
            previous[index++] = interval == NULL ? 0 : interval->reorder;
        }
    }
    iperf_stats_callback(test);
    index = 0;
    SLIST_FOREACH(stream, &test->streams, streams) {
        if (stream->sender) {
            interval = TAILQ_LAST(&stream->result->interval_results, irlisthead);
            long delta = iperf3rs_reorder_delta(interval->reorder, previous[index++]);
            interval_reorder_available |=
                iperf3rs_add_nonnegative(&interval_reorder_events, delta);
        }
    }
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
    double tcp_retransmits = 0.0;
    double tcp_rtt_seconds = 0.0;
    double tcp_rttvar_seconds = 0.0;
    double tcp_snd_cwnd_bytes = 0.0;
    double tcp_snd_wnd_bytes = 0.0;
    double tcp_pmtu_bytes = 0.0;
    double tcp_reorder_events = interval_reorder_events;
    double udp_packets = 0.0;
    double udp_lost_packets = 0.0;
    double udp_jitter_seconds = 0.0;
    double udp_out_of_order_packets = 0.0;
    double omitted = 0.0;
    double interval_duration = 0.0;
    int protocol = 0;
    int direction = 0;
    int matched_streams = 0;
    int tcp_rtt_count = 0;
    int tcp_rttvar_count = 0;
    int tcp_snd_cwnd_count = 0;
    int tcp_snd_wnd_count = 0;
    int tcp_pmtu_count = 0;
    int tcp_retransmits_available = 0;
    int tcp_rtt_seconds_available = 0;
    int tcp_rttvar_seconds_available = 0;
    int tcp_snd_cwnd_bytes_available = 0;
    int tcp_snd_wnd_bytes_available = 0;
    int tcp_pmtu_bytes_available = 0;
    int tcp_reorder_events_available = interval_reorder_available;
    int udp_packets_available = 0;
    int udp_lost_packets_available = 0;
    int udp_jitter_seconds_available = 0;
    int udp_out_of_order_packets_available = 0;
    int interval_ok = 0;
    int stream_must_be_sender;

    if (interval_metrics_callback == NULL) {
        return;
    }

    /*
     * Emit one aggregate direction per callback. In bidirectional mode this
     * keeps the client-side metrics aligned with its sending streams and the
     * server-side metrics aligned with its receiving streams. Emitting both
     * halves would require a wider Rust callback and Prometheus/file schema.
     */
    stream_must_be_sender = iperf3rs_stream_must_be_sender(test);
    direction = stream_must_be_sender ? 1 : 2;

    if (test->protocol->id == Ptcp) {
        protocol = 1;
    } else if (test->protocol->id == Pudp) {
        protocol = 2;
    } else if (test->protocol->id == Psctp) {
        protocol = 3;
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
        if (interval->omitted) {
            omitted = 1.0;
        }
        if (test->protocol->id == Ptcp) {
            if (test->sender_has_retransmits == 1 && stream_must_be_sender) {
                /* TCP_INFO values are only meaningful on the sending stream. */
                tcp_retransmits_available = 1;
                tcp_retransmits += (double)interval->interval_retrans;
                tcp_rtt_count +=
                    iperf3rs_add_nonnegative(&tcp_rtt_seconds, interval->rtt);
                tcp_rttvar_count +=
                    iperf3rs_add_nonnegative(&tcp_rttvar_seconds, interval->rttvar);
                if (interval->snd_cwnd > 0) {
                    tcp_snd_cwnd_bytes += (double)interval->snd_cwnd;
                    tcp_snd_cwnd_count += 1;
                }
                if (interval->snd_wnd > 0) {
                    tcp_snd_wnd_bytes += (double)interval->snd_wnd;
                    tcp_snd_wnd_count += 1;
                }
                tcp_pmtu_count +=
                    iperf3rs_add_nonnegative(&tcp_pmtu_bytes, interval->pmtu);
            }
        } else if (test->protocol->id == Pudp) {
            /* UDP has packet-level interval counters; TCP is reported as bytes. */
            udp_packets_available = 1;
            udp_lost_packets_available = 1;
            udp_out_of_order_packets_available = 1;
            udp_packets += (double)interval->interval_packet_count;
            udp_lost_packets += (double)interval->interval_cnt_error;
            udp_out_of_order_packets += (double)interval->interval_outoforder_packets;
            if (!stream_must_be_sender) {
                udp_jitter_seconds_available = 1;
                udp_jitter_seconds += interval->jitter;
            }
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
    if (test->protocol->id == Pudp && !stream_must_be_sender) {
        udp_jitter_seconds /= matched_streams;
    }
    if (tcp_rtt_count > 0) {
        tcp_rtt_seconds_available = 1;
        tcp_rtt_seconds = tcp_rtt_seconds / tcp_rtt_count / 1000000.0;
    }
    if (tcp_rttvar_count > 0) {
        tcp_rttvar_seconds_available = 1;
        tcp_rttvar_seconds = tcp_rttvar_seconds / tcp_rttvar_count / 1000000.0;
    }
    if (tcp_snd_cwnd_count > 0) {
        tcp_snd_cwnd_bytes_available = 1;
        tcp_snd_cwnd_bytes /= tcp_snd_cwnd_count;
    }
    if (tcp_snd_wnd_count > 0) {
        tcp_snd_wnd_bytes_available = 1;
        tcp_snd_wnd_bytes /= tcp_snd_wnd_count;
    }
    if (tcp_pmtu_count > 0) {
        tcp_pmtu_bytes_available = 1;
        tcp_pmtu_bytes /= tcp_pmtu_count;
    }

    interval_metrics_callback(
        test,
        bytes,
        bandwidth_bits_per_second,
        tcp_retransmits,
        tcp_rtt_seconds,
        tcp_rttvar_seconds,
        tcp_snd_cwnd_bytes,
        tcp_snd_wnd_bytes,
        tcp_pmtu_bytes,
        tcp_reorder_events,
        udp_packets,
        udp_lost_packets,
        udp_jitter_seconds,
        udp_out_of_order_packets,
        interval_duration,
        omitted,
        protocol,
        direction,
        matched_streams,
        tcp_retransmits_available,
        tcp_rtt_seconds_available,
        tcp_rttvar_seconds_available,
        tcp_snd_cwnd_bytes_available,
        tcp_snd_wnd_bytes_available,
        tcp_pmtu_bytes_available,
        tcp_reorder_events_available,
        udp_packets_available,
        udp_lost_packets_available,
        udp_jitter_seconds_available,
        udp_out_of_order_packets_available);
}

static int
iperf3rs_add_nonnegative(double *sum, long value)
{
    if (value < 0) {
        return 0;
    }
    *sum += (double)value;
    return 1;
}

int
iperf3rs_run_server_once(struct iperf_test *test)
{
    int rc = iperf_run_server(test);
    test->server_last_run_rc = rc;
    if (rc < 0 && test->json_output && test->json_top != NULL) {
        if (iperf3rs_cli_interrupted())
            iperf_err(test, "interrupt - %s by signal %s(%d)", iperf_strerror(i_errno), strsignal(iperf3rs_cli_interrupted()), iperf3rs_cli_interrupted());
        else
            iperf_err(test, "error - %s", iperf_strerror(i_errno));
        if (iperf_json_finish(test) < 0) {
            return -2;
        }
        iflush(test);
    }
    return rc;
}

int
iperf3rs_suppress_output(struct iperf_test *test)
{
#ifdef _WIN32
    const char *null_path = "NUL";
#else
    const char *null_path = "/dev/null";
#endif
    char *logfile = NULL;
    size_t logfile_len = 0;

    if (test == NULL) {
        return -1;
    }

    logfile_len = strlen(null_path) + 1;
    logfile = malloc(logfile_len);
    if (logfile == NULL) {
        return -1;
    }
    memcpy(logfile, null_path, logfile_len);

    if (test->logfile != NULL) {
        free(test->logfile);
    }
    test->logfile = logfile;
    return 0;
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

struct iperf3rs_sigpipe_state {
#ifdef SIGPIPE
    struct sigaction previous;
#else
    char unused;
#endif
};

void *
iperf3rs_ignore_sigpipe(void)
{
    struct iperf3rs_sigpipe_state *saved = malloc(sizeof(*saved));
    if (saved == NULL) {
        return NULL;
    }
#ifdef SIGPIPE
    struct sigaction ignored;
    memset(&ignored, 0, sizeof(ignored));
    ignored.sa_handler = SIG_IGN;
    sigemptyset(&ignored.sa_mask);
    if (sigaction(SIGPIPE, &ignored, &saved->previous) < 0) {
        int error_number = errno;
        free(saved);
        errno = error_number;
        return NULL;
    }
#endif
    return saved;
}

int
iperf3rs_restore_sigpipe(void *value)
{
    struct iperf3rs_sigpipe_state *saved = value;
    int rc = 0;
#ifdef SIGPIPE
    rc = sigaction(SIGPIPE, &saved->previous, NULL);
#endif
    int error_number = errno;
    free(saved);
    errno = error_number;
    return rc;
}

#ifdef SIGPIPE
static void
iperf3rs_probe_sigpipe_handler(int signal_number)
{
    (void)signal_number;
}
#endif

/* The regression invokes this only in an isolated test process. */
int
iperf3rs_sigpipe_probe(int install)
{
#ifdef SIGPIPE
    struct sigaction action;
    if (install) {
        memset(&action, 0, sizeof(action));
        action.sa_handler = iperf3rs_probe_sigpipe_handler;
        sigemptyset(&action.sa_mask);
        return sigaction(SIGPIPE, &action, NULL);
    }
    if (sigaction(SIGPIPE, NULL, &action) < 0) {
        return -1;
    }
    return action.sa_handler == iperf3rs_probe_sigpipe_handler;
#else
    return install ? 0 : 1;
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

/* Exposes the borrowed argument for the FFI lifetime regression check. */
const char *
iperf3rs_diskfile_name(struct iperf_test *test)
{
    return test->diskfile_name;
}

/* Allocation accounting is enabled only by an isolated regression process. */
static int iperf3rs_json_allocations;

static void *
iperf3rs_json_alloc(size_t size)
{
    void *value = malloc(size);
    if (value != NULL) iperf3rs_json_allocations++;
    return value;
}

static void
iperf3rs_json_free(void *value)
{
    if (value != NULL) iperf3rs_json_allocations--;
    free(value);
}

int
iperf3rs_json_probe(int install)
{
    if (install) {
        cJSON_Hooks hooks = { iperf3rs_json_alloc, iperf3rs_json_free };
        iperf3rs_json_allocations = 0;
        cJSON_InitHooks(&hooks);
    }
    return iperf3rs_json_allocations;
}

int
iperf3rs_json_probe_session(struct iperf_test *test, int finish)
{
    if (iperf_json_start(test) < 0) return -1;
    if (finish && iperf_json_finish(test) < 0) return -1;
    return 0;
}

/* Two ordinary sessions on the same native owner: incomplete exchange, success. */
int
iperf3rs_server_json_sessions_probe(int port, int reset_ready)
{
    struct iperf_test *test = iperf_new_test();
    char *retained_error = NULL;
    int result = -1;
    if (test == NULL) return -1;
    if (iperf_defaults(test) < 0 || iperf3rs_suppress_output(test) < 0) goto done;
    iperf_set_test_role(test, 's');
    test->server_port = port;
    test->one_off = 1;
    test->json_output = 1;
    if (iperf3rs_run_server_once(test) >= 0 || test->json_output_string == NULL ||
        strstr(test->json_output_string, "\"error\"") == NULL) goto done;
    retained_error = strdup(test->json_output_string);
    if (retained_error == NULL) goto done;
    iperf_reset_test(test);
    if (test->json_top != NULL || test->json_output_string != NULL) goto done;
    /* The previous listener is closed: only a new-listen startup race remains. */
    if (write(reset_ready, "R", 1) != 1) goto done;
    if (iperf3rs_run_server_once(test) < 0 || test->json_output_string == NULL ||
        strstr(test->json_output_string, "\"end\"") == NULL ||
        strstr(test->json_output_string, "\"error\"") != NULL) goto done;
    if (strstr(retained_error, "\"error\"") == NULL) goto done;
    result = 0;
done:
    free(retained_error);
    iperf_free_test(test);
    return result;
}
