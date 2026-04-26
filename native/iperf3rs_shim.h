#ifndef IPERF3RS_SHIM_H
#define IPERF3RS_SHIM_H

#include "iperf_api.h"

typedef void (*iperf3rs_metrics_callback)(
    struct iperf_test *,
    double bytes,
    double bits_per_second,
    double tcp_retransmits,
    double tcp_rtt_seconds,
    double tcp_rttvar_seconds,
    double tcp_snd_cwnd_bytes,
    double tcp_snd_wnd_bytes,
    double tcp_pmtu_bytes,
    double tcp_reorder_events,
    double udp_packets,
    double udp_lost_packets,
    double udp_jitter_seconds,
    double udp_out_of_order_packets,
    double interval_duration_seconds,
    double omitted,
    int protocol,
    int direction,
    int stream_count,
    int tcp_retransmits_available,
    int tcp_rtt_seconds_available,
    int tcp_rttvar_seconds_available,
    int tcp_snd_cwnd_bytes_available,
    int tcp_snd_wnd_bytes_available,
    int tcp_pmtu_bytes_available,
    int tcp_reorder_events_available,
    int udp_packets_available,
    int udp_lost_packets_available,
    int udp_jitter_seconds_available,
    int udp_out_of_order_packets_available);

void iperf3rs_enable_interval_metrics(struct iperf_test *test, iperf3rs_metrics_callback callback);
int iperf3rs_run_server_once(struct iperf_test *test);
int iperf3rs_suppress_output(struct iperf_test *test);
int iperf3rs_current_errno(void);
int iperf3rs_is_auth_test_error(void);
const char *iperf3rs_current_error(void);
void iperf3rs_ignore_sigpipe(void);
char *iperf3rs_usage_long(void);
void iperf3rs_free_string(char *value);

#endif
