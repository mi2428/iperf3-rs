#ifndef IPERF3RS_SHIM_H
#define IPERF3RS_SHIM_H

#include "iperf_api.h"

typedef void (*iperf3rs_json_callback)(struct iperf_test *, char *);

void iperf3rs_enable_json_stream(struct iperf_test *test);
void iperf3rs_set_json_callback(struct iperf_test *test, iperf3rs_json_callback callback);
int iperf3rs_run_server_once(struct iperf_test *test);
int iperf3rs_current_errno(void);
int iperf3rs_is_auth_test_error(void);
const char *iperf3rs_current_error(void);
void iperf3rs_ignore_sigpipe(void);
void iperf3rs_print_usage_long(void);

#endif
