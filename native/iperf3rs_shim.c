#define _GNU_SOURCE

#include "iperf_config.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>

#include "iperf.h"
#include "iperf_api.h"
#include "iperf3rs_shim.h"

void
iperf3rs_enable_json_stream(struct iperf_test *test)
{
    iperf_set_test_json_output(test, 1);
    iperf_set_test_json_stream(test, 1);
}

void
iperf3rs_set_json_callback(struct iperf_test *test, iperf3rs_json_callback callback)
{
    iperf_set_test_json_callback(test, callback);
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
