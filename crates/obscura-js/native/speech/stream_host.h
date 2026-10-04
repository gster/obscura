#pragma once
#import <dispatch/dispatch.h>
#include <memory>
#include <cstdint>
#include <limits>
// Cumulative wire budget. All serialization/writes run on the main sequence.
static bool write_frame(NSDictionary *frame, NSUInteger *written) {
    NSError *error = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:frame options:0 error:&error];
    if (!data || error || data.length >= kMaxTotalBytes - *written) return false;
    *written += data.length + 1;
    return write_all(data.bytes, data.length) && write_all("\n", 1);
}
struct StartupStream : std::enable_shared_from_this<StartupStream> {
    CFRunLoopRef loop = CFRunLoopGetMain();
    dispatch_queue_t worker = dispatch_queue_create("obscura.speech.default", DISPATCH_QUEUE_SERIAL);
    bool completed = false, received_voices_request = false;
    bool updating = false, needs_reupdate = false, default_resolved = false;
    bool initial_written = false, all_queries_background = true;
    int result = 70;
    NSUInteger written = 0;
    uint32_t queries_started = 0, queries_completed = 0, next_revision = 0;
    AVSpeechSynthesisVoice *current_default = nil;
    NSString *branch = @"pending";
    NSString *identifier = nil;
    NSDictionary *cached = nil;

    void finish(int code) { completed = true; result = code; CFRunLoopStop(loop); }
    void guarded(void (^operation)(void)) noexcept {
        if (completed) return;
        try { @try { operation(); }
            @catch (NSException *exception) { (void)exception; finish(2); }
        } catch (...) { finish(3); }
    }
    // Exact coalescing behavior of tts_mac.mm:374-385. No query-count success cap.
    void update_default() {
        if (updating) { needs_reupdate = true; return; }
        if (queries_started == std::numeric_limits<uint32_t>::max()) { finish(70); return; }
        updating = true; ++queries_started;
        const uint32_t query = queries_started;
        auto self = shared_from_this();
        CFRunLoopRef main_loop = loop;
        dispatch_async(worker, ^{
            @autoreleasepool {
                AVSpeechSynthesisVoice *selected = nil;
                NSString *selected_branch = nil;
                NSString *selected_identifier = nil;
                const bool background = !pthread_main_np();
                int failure = 0;
                try { @try {
                    NSUInteger bytes = 0;
                    selected = select_default(&selected_branch, &bytes);
                    if (selected) selected_identifier = checked_string(selected.identifier, &bytes);
                } @catch (NSException *exception) { (void)exception; failure = 2; }
                } catch (...) { failure = 3; }
                // No StartupStream fields are accessed from this background block.
                // Strong captures retain actual native identity and scalar results.
                CFRunLoopPerformBlock(main_loop, kCFRunLoopDefaultMode, ^{
                    @autoreleasepool {
                        self->guarded(^{
                            if (failure) { self->finish(failure); return; }
                            self->on_default(query, selected, selected_branch, selected_identifier, background);
                        });
                    }
                });
                CFRunLoopWakeUp(main_loop);
            }
        });
    }
    // Exact received/cache/nil-default request points in Voices():114-135.
    void voices() {
        if (!received_voices_request) {
            received_voices_request = true;
            update_default();
        }
        if (completed || (cached && [cached[@"mapped_count"] unsignedIntegerValue] != 0)) return;
        auto self = shared_from_this();
        cached = inventory(current_default, branch, @"startup_current_default", ^{
            self->update_default(); // after native enumeration/sort, before mapping
        });
    }
    void snapshot(bool initial, NSString *previous_identifier = nil,
                  bool object_equal = false, id identifier_equal = nil) {
        if (completed) return;
        if (next_revision == std::numeric_limits<uint32_t>::max()) { finish(70); return; }
        NSMutableDictionary *frame = [@{@"schema":@4, @"kind":initial ? @"initial" : @"default_changed",
            @"revision":@(next_revision), @"snapshot":cached,
            @"default_state":default_resolved ? @"resolved" : @"pending",
            @"query_pending":@(updating || needs_reupdate), @"queries_completed":@(queries_completed)} mutableCopy];
        if (!initial) frame[@"transition"] = @{
            @"previous_identifier":previous_identifier ?: (id)NSNull.null,
            @"object_equal":@(object_equal), @"identifier_equal":identifier_equal ?: (id)NSNull.null};
        if (!write_frame(frame, &written)) { finish(4); return; }
        ++next_revision;
    }
    void maybe_terminal() {
        if (completed || !initial_written || updating || needs_reupdate) return;
        if (!default_resolved || queries_started != queries_completed) { finish(70); return; }
        NSDictionary *frame = @{@"schema":@4, @"kind":@"terminal", @"next_revision":@(next_revision),
            @"queries_started":@(queries_started), @"queries_completed":@(queries_completed),
            @"default_background_thread":@(all_queries_background),
            @"default_selection":@{@"branch":branch, @"native_identifier":identifier ?: (id)NSNull.null}};
        finish(write_frame(frame, &written) ? 0 : 4);
    }
    void on_default(uint32_t query, AVSpeechSynthesisVoice *selected, NSString *selected_branch,
                    NSString *selected_identifier, bool background) {
        if (!updating || query != queries_completed + 1 || query != queries_started) { finish(70); return; }
        // Must clear updating BEFORE Voices(), which itself may start a new query.
        updating = false; ++queries_completed;
        const bool object_equal = current_default == selected;
        const bool both_present = current_default && selected;
        const bool identifier_equal = both_present && [current_default.identifier isEqualToString:selected.identifier];
        const bool changed = !object_equal && (!current_default || !selected || !identifier_equal);
        NSString *previous_identifier = identifier;
        current_default = selected; branch = selected_branch; identifier = selected_identifier;
        default_resolved = true; all_queries_background = all_queries_background && background;
        if (changed) {
            cached = nil;
            voices();
            // Early native changes populate the current cache before first observer delivery.
            if (initial_written) snapshot(false, previous_identifier, object_equal,
                both_present ? @(identifier_equal) : (id)NSNull.null);
        }
        if (completed) return;
        // Do not replace this with a fixed second query. update_default may have
        // already started from Voices(nil); then this request is coalesced again.
        if (needs_reupdate) { needs_reupdate = false; update_default(); }
        maybe_terminal();
    }
    void first_request() {
        voices();
        snapshot(true);
        if (completed) return;
        initial_written = true;
        maybe_terminal();
    }
};
static int stream_main_body(void) {
    if (!pthread_main_np()) return 70;
    @autoreleasepool {
        auto state = std::make_shared<StartupStream>();
        if (!state->worker) return 70;
        CFRunLoopSourceContext context = {}; context.perform = keep_alive;
        CFRunLoopSourceRef source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &context);
        if (!source) return 70;
        CFRunLoopAddSource(state->loop, source, kCFRunLoopDefaultMode);
        state->guarded(^{ state->update_default(); }); // constructor request
        CFRunLoopPerformBlock(state->loop, kCFRunLoopDefaultMode, ^{
            @autoreleasepool { state->guarded(^{ state->first_request(); }); }
        });
        CFRunLoopWakeUp(state->loop);
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 7.0, false);
        CFRunLoopRemoveSource(state->loop, source, kCFRunLoopDefaultMode);
        CFRelease(source);
        // Parent's unchanged absolute deadline remains the native-hang backstop.
        return state->completed ? state->result : 5;
    }
}

static int stream_main(void) noexcept {
    try { @try { return stream_main_body(); }
        @catch (NSException *exception) { (void)exception; return 2; }
    } catch (...) { return 3; }
}
