// Window settings Tauri doesn't offer: the region overlay sits above the menu bar and the
// Dock, on the current Space, without a shadow.

#import <AppKit/AppKit.h>

#include "recorder.h"

void kr_overlay_window(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    window.level = NSScreenSaverWindowLevel;
    window.hasShadow = NO;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary |
                                NSWindowCollectionBehaviorStationary;
}

// The window's number (its CGWindowID), for recording one of the app's own windows (the
// test version's pattern window).
uint32_t kr_window_number(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    return window ? (uint32_t)window.windowNumber : 0;
}
