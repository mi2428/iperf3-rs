#define _GNU_SOURCE
#include "iperf_config.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <sched.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "iperf.h"
#include "iperf_api.h"
#include "iperf3rs_cli.h"

struct iperf3rs_cli_state {
    int pipefd[2];
    struct sigaction previous[3];
    int installed;
    char *pidfile;
    int owned;
    struct stat identity;
};

static const int cli_signals[] = { SIGINT, SIGTERM, SIGHUP };
static _Atomic int cli_signal;
static _Atomic int cli_writefd = -1;
static _Atomic unsigned int cli_handlers;
static int cli_readfd = -1;

/* The optional observer exists only for isolated, caller-owned test fixtures. */
static int
iperf3rs_pidfile_checked(const char *path, int *owned, struct stat *identity,
                        int (*observe)(int, const char *))
{
    struct stat observed;
    char buf[sizeof(pid_t) * 3 + 3];
    int fd = -1;
    int existed = 0;
    int saved;
    *owned = 0;
    if (path == NULL) return 0;
    int injected = observe == NULL ? 0 : observe(1, path);
    if (injected) { errno = injected; return -1; }
    fd = open(path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK);
    if (fd >= 0) {
        existed = 1;
        if (fstat(fd, &observed) < 0) goto fail;
        if (!S_ISREG(observed.st_mode)) { errno = EINVAL; goto fail; }
        injected = observe == NULL ? 0 : observe(2, path);
        if (injected) { errno = injected; goto fail; }
        ssize_t length = read(fd, buf, sizeof(buf) - 1);
        if (length < 0) goto fail;
        char extra;
        ssize_t remaining = read(fd, &extra, 1);
        if (remaining < 0) goto fail;
        if (remaining != 0 || length == 0) { errno = EINVAL; goto fail; }
        if (buf[length - 1] == '\n') length--;
        if (length == 0) { errno = EINVAL; goto fail; }
        buf[length] = '\0';
        for (ssize_t index = 0; index < length; index++) {
            if (buf[index] < '0' || buf[index] > '9') { errno = EINVAL; goto fail; }
        }
        errno = 0;
        long value = strtol(buf, NULL, 10);
        pid_t pid = (pid_t)value;
        if (errno == ERANGE || value <= 0 || pid <= 0 || (long)pid != value) {
            errno = EINVAL; goto fail;
        }
        injected = observe == NULL ? 0 : observe(3, path);
        if (injected) errno = injected;
        else if (kill(pid, 0) == 0) { errno = EEXIST; goto fail; }
        if (errno != ESRCH) goto fail; /* EPERM/unknown is not evidence of stale. */
        int closed = close(fd);
        fd = -1;
        if (closed < 0) return -1;
    } else if (errno != ENOENT) return -1;

    injected = observe == NULL ? 0 : observe(4, path);
    if (injected) { errno = injected; return -1; }
    fd = open(path, O_WRONLY | O_NOFOLLOW | O_NONBLOCK | (existed ? 0 : O_CREAT | O_EXCL), S_IRUSR | S_IWUSR);
    if (fd < 0) return -1;
    if (fstat(fd, identity) < 0) goto fail;
    if (existed && (identity->st_dev != observed.st_dev || identity->st_ino != observed.st_ino)) {
        errno = EBUSY; goto fail;
    }
    if (!S_ISREG(identity->st_mode)) { errno = EINVAL; goto fail; }
    if (existed && ftruncate(fd, 0) < 0) goto fail;
    *owned = 1;
    int length = snprintf(buf, sizeof(buf), "%ld", (long)getpid());
    if (length <= 0 || (size_t)length >= sizeof(buf)) { errno = EOVERFLOW; goto fail; }
    ssize_t written = write(fd, buf, (size_t)length);
    if (written < 0) goto fail;
    if (written != length) { errno = EIO; goto fail; }
    return close(fd);
fail:
    saved = errno;
    if (fd >= 0) (void)close(fd);
    errno = saved;
    return -1;
}

int
iperf3rs_create_pidfile(struct iperf_test *test, int *owned, struct stat *identity)
{
    return iperf3rs_pidfile_checked(test->pidfile, owned, identity, NULL);
}

static _Thread_local int pidfile_test_mode;

static int
iperf3rs_pidfile_observe(int stage, const char *path)
{
    if (stage == 1 && pidfile_test_mode == 1) return EACCES;
    if (stage == 2 && pidfile_test_mode == 2) return EIO;
    if (stage == 3 && pidfile_test_mode == 3) return EPERM;
    if (stage == 3 && pidfile_test_mode >= 4) return ESRCH;
    if (stage == 4 && pidfile_test_mode == 5) {
        size_t length = strlen(path);
        char *observed = malloc(length + sizeof(".observed"));
        char *replacement = malloc(length + sizeof(".replacement"));
        if (observed == NULL || replacement == NULL) {
            free(observed); free(replacement); return ENOMEM;
        }
        sprintf(observed, "%s.observed", path);
        sprintf(replacement, "%s.replacement", path);
        int result = rename(path, observed);
        if (result == 0) result = rename(replacement, path);
        int error = result < 0 ? errno : 0;
        free(observed); free(replacement);
        return error;
    }
    return 0;
}

/* Only tests call this; observer never probes an unknown/foreign process. */
int
iperf3rs_pidfile_probe(const char *path, int mode)
{
    struct stat identity;
    int owned;
    pidfile_test_mode = mode;
    int result = iperf3rs_pidfile_checked(path, &owned, &identity, iperf3rs_pidfile_observe);
    int error = result < 0 ? errno : 0;
    pidfile_test_mode = 0;
    return error;
}

static void
iperf3rs_cli_signal(int signal_number)
{
    int saved = errno;
    char byte = 1;
    atomic_fetch_add(&cli_handlers, 1);
    cli_signal = signal_number;
    int fd = atomic_load(&cli_writefd);
    if (fd >= 0) (void)write(fd, &byte, 1);
    atomic_fetch_sub(&cli_handlers, 1);
    errno = saved;
}

int
iperf3rs_cli_interrupted(void)
{
    return cli_readfd < 0 ? 0 : atomic_load(&cli_signal);
}

int
iperf3rs_cli_select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds, struct timeval *timeout)
{
    if (cli_readfd < 0) return select(nfds, readfds, writefds, exceptfds, timeout);
    if (cli_signal) { errno = EINTR; return -1; }
    fd_set empty;
    if (readfds == NULL) { FD_ZERO(&empty); readfds = &empty; }
    FD_SET(cli_readfd, readfds);
    if (nfds <= cli_readfd) nfds = cli_readfd + 1;
    int result = select(nfds, readfds, writefds, exceptfds, timeout);
    FD_CLR(cli_readfd, readfds);
    if (cli_signal) { errno = EINTR; return -1; }
    return result;
}

int
iperf3rs_cli_cleanup(void *value)
{
    struct iperf3rs_cli_state *state = value;
    int result = 0;
    int saved = 0;
    for (int index = state->installed - 1; index >= 0; index--) {
        if (sigaction(cli_signals[index], &state->previous[index], NULL) < 0) {
            result = -1; saved = errno;
        }
    }
    // Caller has joined native/reporter/delivery workers. In-flight handlers
    // still cannot use an fd after close/reuse: retire it, then drain handlers.
    atomic_store(&cli_writefd, -1);
    while (atomic_load(&cli_handlers) != 0) sched_yield();
    cli_readfd = -1;
    cli_signal = 0;
    if (state->pipefd[0] >= 0) close(state->pipefd[0]);
    if (state->pipefd[1] >= 0) close(state->pipefd[1]);
    if (state->owned && state->pidfile != NULL) {
        struct stat current;
        if (lstat(state->pidfile, &current) < 0) {
            if (errno != ENOENT) { result = -1; saved = errno; }
        } else if (current.st_dev != state->identity.st_dev || current.st_ino != state->identity.st_ino) {
            result = -1; saved = EBUSY; /* Never remove a replacement owned elsewhere. */
        } else if (unlink(state->pidfile) < 0) { result = -1; saved = errno; }
    }
    free(state->pidfile);
    free(state);
    if (result < 0) errno = saved;
    return result;
}

void *
iperf3rs_cli_prepare(struct iperf_test *test)
{
    if (cli_readfd >= 0) { errno = EBUSY; return NULL; }
    if (!atomic_is_lock_free(&cli_signal) || !atomic_is_lock_free(&cli_writefd) ||
        !atomic_is_lock_free(&cli_handlers)) { errno = ENOTSUP; return NULL; }
    if (test->role == 's' && test->daemon && daemon(1, 0) < 0) {
        i_errno = IEDAEMON; return NULL;
    }
    struct iperf3rs_cli_state *state = calloc(1, sizeof(*state));
    if (state == NULL) return NULL;
    state->pipefd[0] = state->pipefd[1] = -1;
    if (test->pidfile != NULL && (state->pidfile = strdup(test->pidfile)) == NULL) goto fail;
    if (pipe(state->pipefd) < 0) goto fail;
    if (state->pipefd[0] >= FD_SETSIZE) { errno = EMFILE; goto fail; }
    for (int index = 0; index < 2; index++) {
        if (fcntl(state->pipefd[index], F_SETFD, FD_CLOEXEC) < 0 ||
            fcntl(state->pipefd[index], F_SETFL, O_NONBLOCK) < 0) goto fail;
    }
    cli_readfd = state->pipefd[0];
    cli_writefd = state->pipefd[1];
    cli_signal = 0;
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = iperf3rs_cli_signal;
    sigemptyset(&action.sa_mask);
    for (int index = 0; index < 3; index++) {
        if (sigaction(cli_signals[index], &action, &state->previous[index]) < 0) goto fail;
        state->installed++;
    }
    if (iperf3rs_create_pidfile(test, &state->owned, &state->identity) < 0) {
        i_errno = IEPIDFILE; goto fail;
    }
    return state;
fail:
    {
        int saved = errno;
        iperf3rs_cli_cleanup(state);
        errno = saved;
        return NULL;
    }
}
