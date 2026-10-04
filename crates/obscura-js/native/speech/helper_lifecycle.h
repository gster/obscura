#pragma once
// This guard owns no external process: it can only terminate this helper.
// Install before Foundation/native work. Parent supervision still owns kill/reap.
#include <pthread.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>

namespace speech_lifecycle {
static constexpr int kSetupFailure = 71;
static constexpr int kParentGone = 72;
static constexpr int kDeadline = 73;
static constexpr time_t kDeadlineSeconds = 8;
struct Watch {
    pid_t parent;
    timespec started;
};
// Written once before pthread_create; thereafter immutable for process lifetime.
static Watch watch;
static void *monitor(void *) noexcept {
    for (;;) {
        if (getppid() != watch.parent) _exit(kParentGone);
        timespec now{};
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) _exit(kSetupFailure);
        const time_t seconds = now.tv_sec - watch.started.tv_sec;
        if (seconds > kDeadlineSeconds ||
            (seconds == kDeadlineSeconds && now.tv_nsec >= watch.started.tv_nsec))
            _exit(kDeadline);
        const timespec interval{0, 20 * 1000 * 1000};
        // Interrupted sleeps restart the checks, never defer a due deadline.
        if (nanosleep(&interval, nullptr) != 0 && errno != EINTR) _exit(kSetupFailure);
    }
}
static void install() noexcept {
    watch.parent = getppid();
    if (watch.parent <= 1) _exit(kParentGone);
    if (clock_gettime(CLOCK_MONOTONIC, &watch.started) != 0) _exit(kSetupFailure);
    pthread_attr_t attributes;
    if (pthread_attr_init(&attributes) != 0) _exit(kSetupFailure);
    if (pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED) != 0)
        _exit(kSetupFailure);
    pthread_t thread;
    const int created = pthread_create(&thread, &attributes, monitor, nullptr);
    const int destroyed = pthread_attr_destroy(&attributes);
    if (created != 0 || destroyed != 0) _exit(kSetupFailure);
    if (getppid() != watch.parent) _exit(kParentGone);
}
static void start() noexcept {
#if defined(OBSCURA_HELPER_LIFECYCLE_TEST)
    // Private fixture backstop, before any native work. Never in shipped build.
    if (signal(SIGALRM, SIG_DFL) == SIG_ERR) _exit(kSetupFailure);
    alarm(12);
#endif
#if defined(OBSCURA_HELPER_LIFECYCLE_TEST_DISABLE_GUARD) && !defined(OBSCURA_HELPER_LIFECYCLE_TEST)
#error Guard disabling requires the private lifecycle fixture build.
#endif
#if !defined(OBSCURA_HELPER_LIFECYCLE_TEST_DISABLE_GUARD)
    install();
#endif
}
} // namespace speech_lifecycle
