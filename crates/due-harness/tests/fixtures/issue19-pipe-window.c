// Extend only an existing pipe-to-CLOEXEC window in the selected test process.
#include <fcntl.h>
#include <libproc.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static atomic_int selected;

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
    snprintf(path, sizeof(path), "%s/window.json", root);
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
    // B gets an opportunity to exec before this pipe is returned to Rust.
    // The bound also allows the serialized-spawn diagnostic control to proceed.
    snprintf(path, sizeof(path), "%s/b.ready", root);
    int saw_b = 0;
    for (int i = 0; i < 100; i++) {
        if (access(path, F_OK) == 0) { saw_b = 1; break; }
        usleep(10000);
    }
    snprintf(path, sizeof(path), "%s/window-returned", root);
    log = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (log >= 0) { (void)write(log, saw_b ? "b-before-return\n" : "bounded-release\n", 16); close(log); }
    return result;
}

__attribute__((used)) static struct {
    const void *replacement;
    const void *replacee;
} interpose_pipe __attribute__((section("__DATA,__interpose"))) = {
    (const void *)delayed_pipe, (const void *)pipe
};
