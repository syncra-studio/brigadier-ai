// Computer use's setup window: the helper app's card is dragged into a list in System Settings.
// A web page can't drag a file out of its window, so the drag runs here, in AppKit.

#import <AppKit/AppKit.h>

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
