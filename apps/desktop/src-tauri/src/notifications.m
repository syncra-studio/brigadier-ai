// Notification permission: whether Brigadier may show notifications, and asking for it
// (Settings › General).
#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#include <stdint.h>

typedef void (*Permitted)(uint64_t, int32_t);
// Only an .app bundle has a notification center; `tauri dev` runs a bare binary that still
// carries a bundle identifier, and asking there throws.
static BOOL bundled(void) { return [NSBundle.mainBundle.bundleURL.pathExtension isEqualToString:@"app"]; }
// Asks once; afterwards it answers at once with the user's choice.
void brigadier_notice_ask(void) {
    if (!bundled()) return;
    [UNUserNotificationCenter.currentNotificationCenter
        requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                      completionHandler:^(__unused BOOL granted, __unused NSError *error) {}];
}
// Whether Brigadier may show notifications, without asking: -1 not a bundled app, 0 not asked
// yet, 1 off, 2 allowed.
void brigadier_notice_permission(uint64_t ticket, Permitted permitted) {
    if (!bundled()) {
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
