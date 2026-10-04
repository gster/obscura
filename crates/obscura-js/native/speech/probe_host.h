#pragma once
// Shared bounded host. Derived from reviewed raw helper; root execution NOT_RUN.
#import <Foundation/Foundation.h>
#import <CoreFoundation/CoreFoundation.h>
#include <unistd.h>
#include <errno.h>
#include <pthread.h>
#include <stdexcept>
#include <cstring>
static const NSUInteger kMaxVoices = 4096;
static const NSUInteger kMaxStringBytes = 16384;
static const NSUInteger kMaxTotalBytes = 4 * 1024 * 1024;
static void keep_alive(void *) {}
static bool write_all(const void *data, size_t length) {
    const char *bytes = static_cast<const char *>(data);
    while (length) {
        ssize_t n = write(STDOUT_FILENO, bytes, length);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        bytes += n; length -= static_cast<size_t>(n);
    }
    return true;
}
static NSString *checked_string(NSString *value, NSUInteger *total) {
    if (!value || ![value isKindOfClass:NSString.class])
        @throw [NSException exceptionWithName:@"InvalidVoiceMetadata" reason:nil userInfo:nil];
    // Fast conservative UTF-16 bound before requesting a UTF-8 representation.
    if (value.length > kMaxStringBytes)
        @throw [NSException exceptionWithName:@"VoiceStringLimit" reason:nil userInfo:nil];
    NSData *encoded = [value dataUsingEncoding:NSUTF8StringEncoding allowLossyConversion:NO];
    if (!encoded || encoded.length > kMaxStringBytes || encoded.length > kMaxTotalBytes - *total)
        @throw [NSException exceptionWithName:@"VoiceByteLimit" reason:nil userInfo:nil];
    *total += encoded.length;
    return value;
}
static void invalid(NSString *name) { @throw [NSException exceptionWithName:name reason:nil userInfo:nil]; }
static int probe_main(int argc, char **argv, NSDictionary *(^operation)(void)) {
    const char *mode = argc == 2 ? argv[1] : "inventory";
    if (argc > 2 || (strcmp(mode,"inventory") && strcmp(mode,"simulate-hang") &&
                    strcmp(mode,"simulate-objc-exception") && strcmp(mode,"simulate-cpp-exception"))) return 64;
    if (!pthread_main_np()) return 70;
    @autoreleasepool {
        __block int result = 70;
        __block bool completed = false;
        CFRunLoopRef loop = CFRunLoopGetMain();
        CFRunLoopSourceContext context = {}; context.perform = keep_alive;
        CFRunLoopSourceRef source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &context);
        if (!source) return 70;
        CFRunLoopAddSource(loop, source, kCFRunLoopDefaultMode);
        CFRunLoopPerformBlock(loop, kCFRunLoopDefaultMode, ^{
            @autoreleasepool {
                NSDictionary *response = nil;
                try {
                    @try {
                        if (!strcmp(mode,"simulate-hang")) {
#if defined(OBSCURA_HELPER_LIFECYCLE_TEST)
                            static const char ready[] = "SPEECH_LIFECYCLE_HANG_READY\n";
                            if (write(STDERR_FILENO, ready, sizeof(ready) - 1) != sizeof(ready) - 1)
                                _exit(74);
#endif
                            sleep(60);
                        }
                        if (!strcmp(mode,"simulate-objc-exception"))
                            @throw [NSException exceptionWithName:@"InjectedProbeException" reason:nil userInfo:nil];
                        if (!strcmp(mode,"simulate-cpp-exception")) throw std::runtime_error("injected");
                        response = operation();
                        result = 0;
                    } @catch (NSException *exception) {
                        // Do not print arbitrary exception reasons or native user data.
                        (void)exception;
                        response = @{@"schema":@2,@"status":@"objc_exception_or_invalid_inventory"};
                        result = 2;
                    }
                } catch (...) {
                    response = @{@"schema":@2,@"status":@"cpp_exception"};
                    result = 3;
                }
                // Serialization stays behind both Objective-C and C++ boundaries.
                try {
                @try {
                    NSError *error = nil;
                    NSData *json = [NSJSONSerialization dataWithJSONObject:response options:0 error:&error];
                    if (!json || error || json.length > kMaxTotalBytes || !write_all(json.bytes,json.length) || !write_all("\n",1)) result = 4;
                } @catch (NSException *exception) { (void)exception; result = 4; }
                } catch (...) { result = 4; }
                completed = true;
                CFRunLoopStop(loop);
            }
        });
        CFRunLoopWakeUp(loop);
        // This timeout alone cannot interrupt a hung native method. Parent supervision is mandatory.
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 7.0, false);
        CFRunLoopRemoveSource(loop, source, kCFRunLoopDefaultMode);
        CFRelease(source);
        return completed ? result : 5;
    }
}
