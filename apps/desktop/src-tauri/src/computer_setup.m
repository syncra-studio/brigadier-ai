// Computer use's setup window, natively: where it shows, and the helper app's card dragged into a
// list in System Settings.
// A web page can't drag a file out of its window, so the drag runs here, in AppKit.

#import <AppKit/AppKit.h>
#import <objc/runtime.h>

@interface BrigadierHelperDragSource : NSObject <NSDraggingSource>
@end

@implementation BrigadierHelperDragSource
- (NSDragOperation)draggingSession:(NSDraggingSession *)session
    sourceOperationMaskForDraggingContext:(NSDraggingContext)context {
  return context == NSDraggingContextOutsideApplication
             ? NSDragOperationCopy | NSDragOperationLink | NSDragOperationGeneric
             : NSDragOperationNone;
}
@end

// Starts dragging the file at `path` (an app) from under the pointer in `window`, while the
// mouse button is held. Main thread only. Returns 1 when the drag started.
int brigadier_drag_file(void *ns_window, const char *path) {
  NSWindow *window = (__bridge NSWindow *)ns_window;
  NSView *view = window.contentView;
  if (view == nil || path == NULL) return 0;
  NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
  NSPoint inWindow = [window mouseLocationOutsideOfEventStream];
  NSEvent *event = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged
                                      location:inWindow
                                 modifierFlags:0
                                     timestamp:[[NSProcessInfo processInfo] systemUptime]
                                  windowNumber:window.windowNumber
                                       context:nil
                                   eventNumber:0
                                    clickCount:1
                                      pressure:1.0];
  if (event == nil) return 0;
  NSImage *icon = [[NSWorkspace sharedWorkspace] iconForFile:url.path];
  icon.size = NSMakeSize(48, 48);
  NSPoint inView = [view convertPoint:inWindow fromView:nil];
  NSDraggingItem *item = [[NSDraggingItem alloc] initWithPasteboardWriter:url];
  [item setDraggingFrame:NSMakeRect(inView.x - 24, inView.y - 24, 48, 48) contents:icon];
  static BrigadierHelperDragSource *source;
  if (source == nil) source = [BrigadierHelperDragSource new];
  NSDraggingSession *session = [view beginDraggingSessionWithItems:@[ item ]
                                                             event:event
                                                            source:source];
  session.animatesToStartingPositionsOnCancelOrFail = YES;
  return session != nil ? 1 : 0;
}

// The setup window is a non-activating panel: a regular app's plain window can't show on another
// app's full-screen Space while the app is inactive (it stays on its own Space, unpainted,
// whatever its collection behaviour), and such a panel can. Clicking it gives it the keyboard
// without bringing Brigadier's other windows forward.
//
// It has to be a panel from the start. Changing a live window's class breaks the key-value
// observations AppKit and the webview hold on it (each is tied to the class it was made under),
// and removing one then throws, which ends the app. So the window library's next window is
// allocated as a panel class carrying the library's own window variables and methods.

// The library's window class, the panel class standing in for it, and how many of its next
// windows are panels. Main thread only.
static Class library_window;
static Class setup_panel;
static int panels_pending;
static IMP root_alloc;

// `+alloc` of the library's window class: a pending panel's window is a panel.
static void *window_alloc(void *cls, SEL cmd) {
  Class target = (__bridge Class)cls;
  if (panels_pending > 0 && target == library_window) {
    panels_pending--;
    target = setup_panel;
  }
  return ((void *(*)(Class, SEL))root_alloc)(target, cmd);
}

// Builds the panel class and installs the allocation hook, once. Returns NO if the window
// library's class isn't there (no window made yet) or doesn't look as expected.
static BOOL prepare_panels(void) {
  if (setup_panel != Nil) return YES;
  Class window = objc_getClass("TaoWindow");
  if (window == Nil || class_getSuperclass(window) != [NSWindow class]) return NO;
  Class panel = objc_allocateClassPair([NSPanel class], "BrigadierSetupPanel", 0);
  if (panel == Nil) return NO;
  unsigned count = 0;
  BOOL copied = YES;
  Ivar *ivars = class_copyIvarList(window, &count);
  for (unsigned i = 0; i < count && copied; i++) {
    const char *type = ivar_getTypeEncoding(ivars[i]);
    NSUInteger size = 0, alignment = 0;
    NSGetSizeAndAlignment(type, &size, &alignment);
    uint8_t log2_alignment = 0;
    while ((1u << log2_alignment) < alignment) log2_alignment++;
    copied = class_addIvar(panel, ivar_getName(ivars[i]), size, log2_alignment, type);
  }
  free(ivars);
  if (!copied) {
    objc_disposeClassPair(panel);
    return NO;
  }
  Method *methods = class_copyMethodList(window, &count);
  for (unsigned i = 0; i < count; i++) {
    class_addMethod(panel, method_getName(methods[i]), method_getImplementation(methods[i]),
                    method_getTypeEncoding(methods[i]));
  }
  free(methods);
  objc_registerClassPair(panel);
  root_alloc = method_getImplementation(class_getClassMethod([NSObject class], @selector(alloc)));
  library_window = window;
  setup_panel = panel;
  class_addMethod(object_getClass(window), @selector(alloc), (IMP)window_alloc, "@@:");
  return YES;
}

// Makes the window library's next window a panel. Main thread only, right before the setup
// window is made; returns 0 if it can't, and the window is then a plain one.
int brigadier_next_window_is_panel(void) {
  if (!prepare_panels()) return 0;
  panels_pending = 1;
  return 1;
}

// Right after the setup window is made: no other window becomes a panel.
void brigadier_no_panel_pending(void) { panels_pending = 0; }

// Shows the setup window on the Space the user is on, also over a full-screen app, without
// making Brigadier the active app: an agent asked for setup, and the user should see it where
// they work. Returns 1 when the window is a panel (so it shows there). Main thread only.
int brigadier_show_on_active_space(void *ns_window) {
  NSWindow *window = (__bridge NSWindow *)ns_window;
  if ([window isKindOfClass:[NSPanel class]]) {
    NSPanel *panel = (NSPanel *)window;
    panel.styleMask |= NSWindowStyleMaskNonactivatingPanel;
    panel.hidesOnDeactivate = NO;
  }
  window.collectionBehavior |= NSWindowCollectionBehaviorMoveToActiveSpace |
                               NSWindowCollectionBehaviorFullScreenAuxiliary;
  [window orderFrontRegardless];
  return [window isKindOfClass:[NSPanel class]] ? 1 : 0;
}
