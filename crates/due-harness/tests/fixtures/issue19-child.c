// Model-free leader/readiness/sibling commands with file-based barriers.
#include <fcntl.h>
#include <libproc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void put(const char *root, const char *name, const char *value) {
    char path[4096]; snprintf(path, sizeof(path), "%s/%s", root, name);
    FILE *file = fopen(path, "w");
    if (!file) exit(2);
    fputs(value, file); fclose(file);
}
static int exists(const char *root, const char *name) {
    char path[4096]; snprintf(path, sizeof(path), "%s/%s", root, name);
    return access(path, F_OK) == 0;
}
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    const char *root = argv[1], *role = argv[2];
    if (strcmp(role, "arm") == 0) {
        char parent[64];
        snprintf(parent, sizeof(parent), "%d\n", getppid());
        put(root, "parent.pid", parent);
        put(root, "arm", "arm\n");
        return 0;
    }
    if (strcmp(role, "release") == 0) {
        put(root, "release", "release\n");
        return 0;
    }
    char path[4096], name[64], pid[64];
    snprintf(name, sizeof(name), "%s.fds.jsonl", role);
    snprintf(path, sizeof(path), "%s/%s", root, name);
    FILE *file = fopen(path, "w");
    if (!file) return 2;
    struct proc_fdinfo descriptors[1024];
    int size = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, descriptors, sizeof(descriptors));
    if (size <= 0) return 4;
    for (int i = 0; i < size / (int)sizeof(descriptors[0]); i++) {
        int fd = descriptors[i].proc_fd;
        if (descriptors[i].proc_fdtype != PROX_FDTYPE_PIPE) continue;
        struct pipe_fdinfo info = {0};
        int bytes = proc_pidfdinfo(getpid(), fd, PROC_PIDFDPIPEINFO, &info, sizeof(info));
        if (bytes != sizeof(info)) return 5;
        fprintf(file,
            "{\"fd\":%d,\"flags\":%d,\"access\":%d,\"handle\":\"%llx\",\"peer\":\"%llx\"}\n",
            fd, fcntl(fd, F_GETFD), fcntl(fd, F_GETFL) & O_ACCMODE,
            info.pipeinfo.pipe_handle, info.pipeinfo.pipe_peerhandle);
    }
    fclose(file);
    snprintf(pid, sizeof(pid), "%d\n", getpid());
    snprintf(name, sizeof(name), "%s.ready", role); put(root, name, pid);
    if (strcmp(role, "r") == 0) { puts("readiness complete"); return 0; }
    for (int i = 0; i < 3000; i++) {
        if (strcmp(role, "b") == 0 && exists(root, "challenge")) put(root, "b.ack", "fresh challenge answered\n");
        if (strcmp(role, "b") == 0 && exists(root, "release")) return 0;
        usleep(10000);
    }
    return 3;
}
