#include "helper_lifecycle.h"
// Source-only streaming revision. Root reviews, compiles and executes separately.
// No synthesizer instance, utterance, speech, authorization, downloads or writes.
#import <AVFAudio/AVFAudio.h>
#import <AppKit/AppKit.h>
#include "probe_host.h"
#include "mapping_core.h"

// Same selection chain as pinned tts_mac.mm:49-111. No raw preferences exported.
static AVSpeechSynthesisVoice *select_default(NSString **branch, NSUInteger *total) {
    NSUserDefaults *preferences = [[NSUserDefaults alloc]
        initWithSuiteName:@"com.apple.Accessibility"];
    NSArray *settings = [preferences arrayForKey:@"SpokenContentDefaultVoiceSelectionsByLanguage"];
    AVSpeechSynthesisVoice *voice = nil;
    if (settings.count > 1) {
        NSDictionary *selected = settings[1];
        NSString *identifier = selected[@"voiceId"];
        if (identifier) checked_string(identifier, total);
        voice = [AVSpeechSynthesisVoice voiceWithIdentifier:identifier];
        if (voice) { *branch = @"accessibility_voice_id"; return voice; }
    }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    NSString *identifier = NSSpeechSynthesizer.defaultVoice;
#pragma clang diagnostic pop
    if (identifier) checked_string(identifier, total);
    voice = [AVSpeechSynthesisVoice voiceWithIdentifier:identifier];
    if (voice) { *branch = @"ns_speech_default_voice"; return voice; }
    voice = [AVSpeechSynthesisVoice voiceWithLanguage:nil];
    *branch = voice ? @"av_system_language_region" : @"none";
    return voice;
}

static NSDictionary *copy_native_row(AVSpeechSynthesisVoice *voice, NSUInteger *total) {
    NSString *name = voice.name;
    if (!name) return nil;
    return @{@"native_identifier":checked_string(voice.identifier, total),
             @"raw_name":checked_string(name, total),
             @"language":checked_string(voice.language, total)};
}

static NSDictionary *inventory(AVSpeechSynthesisVoice *default_voice, NSString *branch, NSString *scope, void (^no_default)(void)) {
    if (!pthread_main_np() || !NSThread.isMainThread || CFRunLoopGetCurrent() != CFRunLoopGetMain())
        invalid(@"NotMainThread");
    NSUInteger total = 0, raw_skipped = 0, ordered_skipped = 0;
    NSString *default_identifier = default_voice ? checked_string(default_voice.identifier, &total) : nil;
    NSArray<AVSpeechSynthesisVoice *> *voices = AVSpeechSynthesisVoice.speechVoices;
    if (!voices || voices.count > kMaxVoices) invalid(@"VoiceCountLimitOrNil");
    NSMutableArray *raw_rows = [NSMutableArray arrayWithCapacity:voices.count];
    for (AVSpeechSynthesisVoice *voice in voices) {
        NSDictionary *row = copy_native_row(voice, &total);
        if (row) [raw_rows addObject:row]; else ++raw_skipped;
    }
    // Keep exact Foundation name sorting and AV object equality semantics.
    // Never deduplicate/reorder by identifier as a substitute for removeObject:.
    NSMutableArray *ordered = [[voices sortedArrayUsingDescriptors:@[
        [NSSortDescriptor sortDescriptorWithKey:@"name" ascending:YES]
    ]] mutableCopy];
    NSUInteger before_default = ordered.count;
    NSUInteger removed_equal_objects = 0;
    if (default_voice) {
        [ordered removeObject:default_voice];
        removed_equal_objects = before_default - ordered.count;
        if (ordered.count >= kMaxVoices) invalid(@"VoiceCountLimitAfterDefault");
        [ordered insertObject:default_voice atIndex:0];
    } else if (no_default) {
        no_default();
    }
    NSMutableArray *ordered_rows = [NSMutableArray arrayWithCapacity:ordered.count];
    for (AVSpeechSynthesisVoice *voice in ordered) {
        NSDictionary *row = copy_native_row(voice, &total);
        if (row) [ordered_rows addObject:row]; else ++ordered_skipped;
    }
    NSString *locale_before = checked_string(NSLocale.autoupdatingCurrentLocale.localeIdentifier, &total);
    NSArray *mapped = map_ordered_rows(ordered_rows, ^NSString *(NSString *language) {
        return [NSLocale.autoupdatingCurrentLocale localizedStringForLocaleIdentifier:language];
    }, &total);
    NSString *locale_after = checked_string(NSLocale.autoupdatingCurrentLocale.localeIdentifier, &total);
    return @{
        @"schema":@2, @"status":@"native_mapping_observed", @"main_thread":@YES,
        @"main_run_loop":@YES, @"chromium_revision":@"792bf6722e73a45aa9e47c163b9901bdc17f3230",
        @"mapping_scope":scope,
        @"default_selection":@{@"branch":branch,
            @"native_identifier":default_identifier ?: (id)NSNull.null,
            @"removed_equal_av_objects":@(removed_equal_objects)},
        @"locale_before":locale_before, @"locale_after":locale_after,
        @"locale_identifier_changed":@(![locale_before isEqualToString:locale_after]),
        @"native_count":@(voices.count), @"record_count":@(raw_rows.count),
        @"skipped_nil_name":@(raw_skipped), @"ordered_skipped_nil_name":@(ordered_skipped),
        @"mapped_count":@(mapped.count), @"string_bytes_charged":@(total),
        @"synthesis":@"not_invoked", @"preferences":@"read_only_chromium_default_chain",
        @"voices":raw_rows, @"mapped_voices":mapped
    };
}
#include "stream_host.h"
int main(int argc, char **argv) {
    speech_lifecycle::start();
    if (argc == 1 || (argc == 2 && !strcmp(argv[1], "inventory"))) return stream_main();
    return probe_main(argc, argv, ^{
        return inventory(nil, @"pending", @"startup_current_default", nil);
    });
}
