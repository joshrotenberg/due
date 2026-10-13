// Darwin-only fault injection for Rust 1.90's pipe() -> fcntl(FD_CLOEXEC)
// window. Delay return from real pipe(); never clear flags or change data.
// Only the workflow host is perturbed, not its Due run/native children.
#include <crt_externs.h>
#include <fcntl.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static atomic_uint calls;
static _Thread_local unsigned local_calls;

static int widened_pipe(int fds[2]) {
    int result = pipe(fds);
    char **args = *_NSGetArgv();
    const char *log = getenv("DUE_PIPE_WINDOW_LOG");
    if (result == 0 && args && args[1] &&
        strcmp(args[1], "workflow") == 0 && log) {
        unsigned index = atomic_fetch_add(&calls, 1);
        // Each Due spawn makes stdin, stdout, stderr pipes in that order.
        unsigned delay = local_calls++ % 3 == 1 ? 210000 + (index % 3) * 50000 : 0;
        int fd = open(log, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0600);
        if (fd >= 0) {
            char line[160];
            int n = snprintf(line, sizeof(line), "%d %u %d %d %u\n",
                             getpid(), index, fds[0], fds[1], delay);
            (void)write(fd, line, n);
            close(fd);
        }
        usleep(delay);
    }
    return result;
}

__attribute__((used)) static struct {
    const void *replacement;
    const void *replacee;
} interpose_pipe __attribute__((section("__DATA,__interpose"))) = {
    (const void *)widened_pipe, (const void *)pipe
};
