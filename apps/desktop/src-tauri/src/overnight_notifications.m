// A bounded adapter for run notifications. The desktop plugin does not expose activation
// payloads or OS submission results. UserNotifications preserves these across app exit.
#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#include <stdint.h>

typedef void (*Activated)(const char *, const char *, const char *);
typedef void (*Submitted)(uint64_t, const char *);
typedef void (*Permitted)(uint64_t, int32_t);
// What a refusal because notifications are off reports, so Rust can say it plainly.
static const char *const NOTIFICATIONS_OFF = "notifications-off";
static Activated activated;
@interface BrigadierRunNoticeDelegate : NSObject <UNUserNotificationCenterDelegate>
@end
@implementation BrigadierRunNoticeDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
      willPresentNotification:(UNNotification *)notification
        withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
 didReceiveNotificationResponse:(UNNotificationResponse *)response
        withCompletionHandler:(void (^)(void))completion {
    NSDictionary *info = response.notification.request.content.userInfo;
    NSString *conversation = info[@"conversation"];
    NSString *run = info[@"run"];
    if ([response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier]
        && [conversation isKindOfClass:NSString.class] && [run isKindOfClass:NSString.class]
        && activated) {
        activated(conversation.UTF8String, run.UTF8String, response.notification.request.identifier.UTF8String);
    }
    completion();
}
@end
static BrigadierRunNoticeDelegate *delegate;
static NSString *restoredData;
const char *brigadier_notice_data_dir(void) {
    if (!NSBundle.mainBundle.bundleIdentifier) return NULL;
    restoredData = [NSUserDefaults.standardUserDefaults stringForKey:@"BrigadierRunDataDir"];
    return restoredData.UTF8String;
}
void brigadier_notice_init(Activated callback, const char *dataDir) {
    [NSUserDefaults.standardUserDefaults setObject:@(dataDir) forKey:@"BrigadierRunDataDir"];
    if (!NSBundle.mainBundle.bundleIdentifier) return;
    activated = callback;
    delegate = [BrigadierRunNoticeDelegate new];
    UNUserNotificationCenter.currentNotificationCenter.delegate = delegate;
}
// Asks once, while the user starts a run at the Mac, so the prompt isn't left for a morning
// notification to raise; afterwards it answers at once with the user's choice.
void brigadier_notice_ask(void) {
    if (!NSBundle.mainBundle.bundleIdentifier) return;
    [UNUserNotificationCenter.currentNotificationCenter
        requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                      completionHandler:^(__unused BOOL granted, __unused NSError *error) {}];
}
// Whether Brigadier may show notifications, without asking: -1 not a bundled app, 0 not asked
// yet, 1 off, 2 allowed.
void brigadier_notice_permission(uint64_t ticket, Permitted permitted) {
    if (!NSBundle.mainBundle.bundleIdentifier) {
        permitted(ticket, -1);
        return;
    }
    [UNUserNotificationCenter.currentNotificationCenter
        getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
            switch (settings.authorizationStatus) {
            case UNAuthorizationStatusNotDetermined: permitted(ticket, 0); break;
            case UNAuthorizationStatusDenied: permitted(ticket, 1); break;
            default: permitted(ticket, 2); break;
            }
        }];
}
void brigadier_notice_send(const char *identifier, const char *title, const char *body,
                          const char *conversation, const char *run, uint64_t ticket,
                          Submitted submitted) {
    if (!NSBundle.mainBundle.bundleIdentifier) {
        submitted(ticket, "Run notifications require a bundled Brigadier app");
        return;
    }
    // Copy all Rust strings before returning; completion callbacks may run later.
    UNMutableNotificationContent *content = [UNMutableNotificationContent new];
    content.title = @(title);
    content.body = @(body);
    content.userInfo = @{@"conversation": @(conversation), @"run": @(run)};
    UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:@(identifier)
                                                                        content:content trigger:nil];
    UNUserNotificationCenter *center = UNUserNotificationCenter.currentNotificationCenter;
    [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert completionHandler:^(BOOL granted, NSError *error) {
        if (!granted || error) {
            BOOL off = !error || ([error.domain isEqualToString:UNErrorDomain]
                                  && error.code == UNErrorCodeNotificationsNotAllowed);
            submitted(ticket, off ? NOTIFICATIONS_OFF : error.localizedDescription.UTF8String);
            return;
        }
        [center addNotificationRequest:request withCompletionHandler:^(NSError *error) {
            submitted(ticket, error ? error.localizedDescription.UTF8String : NULL);
        }];
    }];
}
