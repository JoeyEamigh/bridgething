#include "BridgethingCrashHooks.h"

#if defined(__APPLE__)

#import <Foundation/Foundation.h>

#include <atomic>
#include <cstdlib>
#include <exception>
#include <mutex>
#include <string>

namespace {

std::atomic<bridgething_crash_sink> crash_sink{nullptr};
std::terminate_handler previous_handler = nullptr;
std::atomic_flag reporting = ATOMIC_FLAG_INIT;

std::string describe(NSException *exception) {
  id untruncated = exception.userInfo[@"RCTUntruncatedMessageKey"];
  NSString *detail = [untruncated isKindOfClass:NSString.class] ? untruncated : exception.reason;
  NSString *frames = [exception.callStackSymbols componentsJoinedByString:@"\n"];
  NSString *text = [NSString stringWithFormat:@"uncaught objc exception %@: %@\n%@", exception.name, detail ?: @"",
                                              frames ?: @""];
  const char *utf8 = text.UTF8String;
  return utf8 ? std::string(utf8) : std::string("uncaught objc exception");
}

std::string describe_current() {
  std::exception_ptr held = std::current_exception();
  if (!held) {
    return "std::terminate without an active exception";
  }
  try {
    @try {
      std::rethrow_exception(held);
    } @catch (NSException *exception) {
      return describe(exception);
    } @catch (id other) {
      return "uncaught objc throwable of unknown type";
    }
  } catch (const std::exception &exception) {
    return std::string("uncaught c++ exception: ") + exception.what();
  } catch (...) {
    return "uncaught exception of unknown type";
  }
}

void on_terminate() {
  if (!reporting.test_and_set()) {
    if (bridgething_crash_sink target = crash_sink.load()) {
      @autoreleasepool {
        std::string text = describe_current();
        target(text.c_str());
      }
    }
  }
  if (previous_handler) {
    previous_handler();
  }
  std::abort();
}

}  // namespace

extern "C" void bridgething_install_crash_hooks(bridgething_crash_sink sink) {
  crash_sink.store(sink);
  static std::once_flag once;
  std::call_once(once, [] { previous_handler = std::set_terminate(on_terminate); });
}

#else

extern "C" void bridgething_install_crash_hooks(bridgething_crash_sink) {}

#endif
