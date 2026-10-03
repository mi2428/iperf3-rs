#ifndef IPERF3RS_CLI_H
#define IPERF3RS_CLI_H

#include <sys/select.h>
#include <sys/stat.h>

struct iperf_test;
void *iperf3rs_cli_prepare(struct iperf_test *test);
int iperf3rs_cli_cleanup(void *state);
int iperf3rs_cli_interrupted(void);
int iperf3rs_cli_select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds, struct timeval *timeout);
void iperf3rs_cli_report_interrupt(struct iperf_test *test);
int iperf3rs_create_pidfile(struct iperf_test *test, int *owned, struct stat *identity);
int iperf3rs_pidfile_probe(const char *path, int mode);

#endif
