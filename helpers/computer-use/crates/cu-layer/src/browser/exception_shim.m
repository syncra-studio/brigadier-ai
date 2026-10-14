#import <CoreServices/CoreServices.h>
#import <Foundation/Foundation.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

static char *cu_copy_utf8(NSString *value) {
    if (value == nil) {
        return NULL;
    }
    const char *utf8 = [value UTF8String];
    return utf8 == NULL ? NULL : strdup(utf8);
}

static void cu_set_error(char **error_out, NSString *message) {
    if (error_out != NULL) {
        *error_out = cu_copy_utf8(message ?: @"Unknown Objective-C error");
    }
}

bool cu_layer_browser_execute_script(
    const char *source_utf8,
    char **result_out,
    char **error_out
) {
    if (result_out != NULL) {
        *result_out = NULL;
    }
    if (error_out != NULL) {
        *error_out = NULL;
    }

    @autoreleasepool {
        @try {
            if (source_utf8 == NULL) {
                cu_set_error(error_out, @"Browser script source was null");
                return false;
            }
            NSString *source = [[NSString alloc] initWithUTF8String:source_utf8];
            if (source == nil) {
                cu_set_error(error_out, @"Browser script source was not valid UTF-8");
                return false;
            }
            NSAppleScript *script = [[NSAppleScript alloc] initWithSource:source];
            if (script == nil) {
                cu_set_error(error_out, @"Could not initialize the in-process browser script");
                return false;
            }

            NSDictionary *script_error = nil;
            NSAppleEventDescriptor *descriptor =
                [script executeAndReturnError:&script_error];
            if (script_error != nil) {
                cu_set_error(error_out, [script_error description]);
                return false;
            }
            if (descriptor == nil) {
                cu_set_error(error_out, @"Browser Apple Event returned no result");
                return false;
            }

            NSString *value = [descriptor stringValue];
            if (value == nil) {
                NSAppleEventDescriptor *coerced =
                    [descriptor coerceToDescriptorType:0x75746638]; // 'utf8'
                value = [coerced stringValue];
            }
            if (result_out != NULL) {
                *result_out = cu_copy_utf8(value ?: @"");
            }
            return true;
        } @catch (id exception) {
            NSString *message = [exception respondsToSelector:@selector(description)]
                ? [exception description]
                : @"Unknown Objective-C exception";
            cu_set_error(error_out, message);
            return false;
        }
    }
}

bool cu_layer_browser_exception_boundary_probe(char **error_out) {
    if (error_out != NULL) {
        *error_out = NULL;
    }
    @try {
        (void)[@[] objectAtIndex:0];
        return true;
    } @catch (id exception) {
        cu_set_error(error_out, [exception description]);
        return false;
    }
}

void cu_layer_browser_free_string(char *value) {
    free(value);
}

// Whether macOS lets this process send Apple Events to the app `bundle_id`
// (Privacy & Security > Automation): noErr, errAEEventNotPermitted (-1743),
// errAEEventWouldRequireUserConsent (-1744) when nobody has answered yet, or
// procNotFound (-600) when the app isn't running. With `ask`, it shows macOS's
// prompt when nobody has answered, and waits for the answer.
int32_t cu_layer_automation_permission(const char *bundle_id, bool ask) {
    @autoreleasepool {
        NSString *bundle = bundle_id == NULL ? nil : [NSString stringWithUTF8String:bundle_id];
        if (bundle == nil) {
            return paramErr;
        }
        NSAppleEventDescriptor *target =
            [NSAppleEventDescriptor descriptorWithBundleIdentifier:bundle];
        return AEDeterminePermissionToAutomateTarget(target.aeDesc, typeWildCard, typeWildCard,
                                                     ask);
    }
}
