#ifndef BRIDGETHING_CRASH_HOOKS_H
#define BRIDGETHING_CRASH_HOOKS_H

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*bridgething_crash_sink)(const char *message);

void bridgething_install_crash_hooks(bridgething_crash_sink sink);

#ifdef __cplusplus
}
#endif

#endif
