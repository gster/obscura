#pragma once
// Private source preparation. Shared scalar mapping stage; no speech APIs here.
// Chromium 792bf6722e73a45aa9e47c163b9901bdc17f3230:
// tts_mac.mm:139-195; speech_synthesis_impl.cc:99-113.
#import <Foundation/Foundation.h>
static NSArray *map_ordered_rows(NSArray *rows,
                                NSString *(^localized_name)(NSString *),
                                NSUInteger *total) {
    if (rows.count > kMaxVoices) invalid(@"VoiceCountLimit");
    NSMutableDictionary<NSString *, NSNumber *> *counts = [NSMutableDictionary dictionary];
    for (NSDictionary *row in rows) {
        NSString *name = row[@"raw_name"];
        // Rows are copied after the same nil-name skip as Chromium.
        counts[name] = @([counts[name] unsignedIntegerValue] + 1);
    }
    NSMutableArray *result = [NSMutableArray arrayWithCapacity:rows.count];
    for (NSDictionary *row in rows) {
        NSString *raw_name = row[@"raw_name"], *name = raw_name;
        NSString *localized = nil;
        if ([counts[raw_name] unsignedIntegerValue] > 1) {
            localized = localized_name(row[@"language"]);
            if (localized) checked_string(localized, total);
            // Preserve NSString %@ formatting, including nil -> (null).
            name = [NSString stringWithFormat:@"%@ (%@)", raw_name, localized];
        }
        name = checked_string(name, total);
        // Web exposes the mapped name as BOTH voiceURI and name.
        checked_string(name, total);
        NSString *identifier = checked_string(row[@"native_identifier"], total);
        NSString *language = checked_string(row[@"language"], total);
        checked_string(raw_name, total);
        [result addObject:@{
            @"native_identifier":identifier, @"raw_name":raw_name,
            @"language":language, @"native":@YES, @"remote":@NO,
            @"name_count":counts[raw_name],
            @"localized_language":localized ?: (id)NSNull.null,
            @"web":@{@"voiceURI":name, @"name":name, @"lang":language,
                     @"localService":@YES, @"default":@(result.count == 0)}
        }];
    }
    return result;
}
