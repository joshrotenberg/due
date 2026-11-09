// Extend only an existing pipe-to-CLOEXEC window in the selected test process.
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <poll.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static atomic_int selected;

// Fail the spawn, closing the exposed pipe so Rust can join its other jobs.
static int window_failure(int fds[2], int release, const char *phase, int error) {
    fprintf(stderr, "R pre-CLOEXEC window: %s: %s\n", phase, strerror(error));
    if (release >= 0) close(release);
    close(fds[0]); close(fds[1]);
    errno = error;
    return -1;
}

static int delayed_pipe(int fds[2]) {
    int result = pipe(fds);
    const char *root = getenv("DUE19_EVIDENCE_DIR");
    if (result != 0 || !root) return result;
    char path[4096];
    snprintf(path, sizeof(path), "%s/parent.pid", root);
    FILE *pid_file = fopen(path, "r");
    int parent = -1;
    if (pid_file) { (void)fscanf(pid_file, "%d", &parent); fclose(pid_file); }
    if (getpid() != parent) return result;
    snprintf(path, sizeof(path), "%s/arm", root);
    if (access(path, F_OK) != 0 || atomic_exchange(&selected, 1)) return result;
    struct pipe_fdinfo read_info = {0}, write_info = {0};
    int read_size = proc_pidfdinfo(getpid(), fds[0], PROC_PIDFDPIPEINFO,
                                 &read_info, sizeof(read_info));
    int write_size = proc_pidfdinfo(getpid(), fds[1], PROC_PIDFDPIPEINFO,
                                  &write_info, sizeof(write_info));
    // Open the explicit release channel before publishing the real pipe window.
    // O_RDWR avoids FIFO EOF/HUP before the test writes its release byte.
    snprintf(path, sizeof(path), "%s/window.release", root);
    int release = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (release < 0) return window_failure(fds, -1, "open release FIFO", errno);
    snprintf(path, sizeof(path), "%s/window.pending", root);
    int log = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (log >= 0) {
        char data[1024];
        int length = snprintf(data, sizeof(data),
            "{\"pid\":%d,\"read_fd\":%d,\"write_fd\":%d,\"read_flags\":%d,"
            "\"write_flags\":%d,\"read_handle\":\"%llx\",\"write_handle\":\"%llx\","
            "\"read_info_bytes\":%d,\"write_info_bytes\":%d}\n",
            getpid(), fds[0], fds[1], fcntl(fds[0], F_GETFD),
            fcntl(fds[1], F_GETFD), read_info.pipeinfo.pipe_handle,
            write_info.pipeinfo.pipe_handle, read_size, write_size);
        (void)write(log, data, (size_t)length);
        close(log);
    }
    char published[4096];
    snprintf(published, sizeof(published), "%s/window.json", root);
    if (log < 0 || rename(path, published) != 0) {
        return window_failure(fds, release, "publish window.json", errno);
    }
    // The test releases R only after B actually attempts the spawn boundary.
    // A watchdog is failure, never permission to return the exposed pipe.
    struct timespec start, now;
    if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) {
        return window_failure(fds, release, "start watchdog", errno);
    }
    struct pollfd wait = { .fd = release, .events = POLLIN };
    for (;;) {
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
            return window_failure(fds, release, "read watchdog clock", errno);
        }
        long remaining = 10000 - (now.tv_sec - start.tv_sec) * 1000
            - (now.tv_nsec - start.tv_nsec) / 1000000;
        if (remaining <= 0) {
            return window_failure(fds, release, "test never released window.release", ETIMEDOUT);
        }
        int event = poll(&wait, 1, (int)remaining);
        if (event < 0 && errno == EINTR) continue;
        if (event <= 0) {
            return window_failure(fds, release, "wait for window.release", event == 0 ? ETIMEDOUT : errno);
        }
        char byte;
        if (!(wait.revents & POLLIN) || read(release, &byte, 1) != 1 || byte != 'R') {
            return window_failure(fds, release, "invalid explicit release", EIO);
        }
        break;
    }
    close(release);
    snprintf(path, sizeof(path), "%s/window-returned", root);
    log = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (log >= 0) { (void)write(log, "explicit-release\n", 17); close(log); }
    return result;
}

__attribute__((used)) static struct {
    const void *replacement;
    const void *replacee;
} interpose_pipe __attribute__((section("__DATA,__interpose"))) = {
    (const void *)delayed_pipe, (const void *)pipe
};
