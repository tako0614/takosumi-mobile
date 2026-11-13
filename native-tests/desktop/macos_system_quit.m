// Fixture-only normal OS quit for one previously attested browser process.
// AppKit's return value means request sent, never successful termination.
#import <AppKit/AppKit.h>
#include <errno.h>
#include <limits.h>
#include <libproc.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static uint64_t number(const char *text) {
    for (const char *p = text; *p; ++p) if (*p < '0' || *p > '9') return 0;
    char *end;
    errno = 0;
    unsigned long long value = strtoull(text, &end, 10);
    return errno || end == text || *end ? 0 : value;
}

static BOOL identity(pid_t pid, pid_t parent, uint64_t start) {
    struct proc_bsdinfo info = {0};
    int size = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info));
    return (size_t)size == sizeof(info) && info.pbi_pid == (uint32_t)pid
        && (parent == 0 || info.pbi_ppid == (uint32_t)parent)
        && info.pbi_start_tvsec * 1000000ULL + info.pbi_start_tvusec == start;
}

int main(int argc, const char *argv[]) {
    if (argc != 5) return 2;
    uint64_t pid = number(argv[1]), parent = number(argv[2]);
    uint64_t start = number(argv[3]), parentStart = number(argv[4]);
    if (!pid || pid > INT_MAX || !parent || parent > INT_MAX || !start || !parentStart) return 2;
    @autoreleasepool {
        NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)pid];
        if (!app || app.processIdentifier != (pid_t)pid || app.terminated
            || !identity((pid_t)pid, (pid_t)parent, start)
            || !identity((pid_t)parent, 0, parentStart)) return 3;
        // No forceTerminate, process-name lookup or signal fallback.
        if (![app terminate]) return 4;
        printf("{\"requested\":true,\"pid\":%llu}\n", (unsigned long long)pid);
    }
    return 0;
}
