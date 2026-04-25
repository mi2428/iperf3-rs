#ifndef IPERF3RS_SHIM_H
#define IPERF3RS_SHIM_H

#include "iperf_api.h"

typedef void (*iperf3rs_metrics_callback)(
    struct iperf_test *,
    double bytes,
    double bits_per_second,
    double packets,
    double error_packets,
    double jitter_seconds,
    double tcp_retransmits);

void iperf3rs_enable_interval_metrics(struct iperf_test *test, iperf3rs_metrics_callback callback);
int iperf3rs_run_server_once(struct iperf_test *test);
int iperf3rs_current_errno(void);
int iperf3rs_is_auth_test_error(void);
const char *iperf3rs_current_error(void);
void iperf3rs_ignore_sigpipe(void);
char *iperf3rs_usage_long(void);
void iperf3rs_free_string(char *value);

#endif
