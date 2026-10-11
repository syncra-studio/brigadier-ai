// Open at login on macOS: the app's own login item (System Settings › General › Login Items),
// through SMAppService, so no helper or launch agent is installed.
#import <Foundation/Foundation.h>
#import <ServiceManagement/ServiceManagement.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

// 0 off, 1 on, 2 waiting for the user's approval in System Settings, 3 not available.
int32_t brigadier_login_item_status(void) {
    switch (SMAppService.mainAppService.status) {
        case SMAppServiceStatusNotRegistered: return 0;
        case SMAppServiceStatusEnabled: return 1;
        case SMAppServiceStatusRequiresApproval: return 2;
        default: return 3;
    }
}

// NULL once done; otherwise why not, which the caller frees with brigadier_login_item_free.
char *brigadier_login_item_set(bool on) {
    NSError *error = nil;
    SMAppService *service = SMAppService.mainAppService;
    BOOL done = on ? [service registerAndReturnError:&error] : [service unregisterAndReturnError:&error];
    if (done) return NULL;
    const char *reason = error.localizedDescription.UTF8String;
    return strdup(reason ? reason : "The login item could not be changed.");
}

void brigadier_login_item_free(char *reason) {
    free(reason);
}

void brigadier_login_item_open_settings(void) {
    [SMAppService openSystemSettingsLoginItems];
}
