// Window settings Tauri doesn't offer: the region overlay sits above the menu bar and the
// Dock, on the current Space, without a shadow.

#import <AppKit/AppKit.h>
#import <objc/message.h>
#import <objc/runtime.h>

#include "recorder.h"

void kr_overlay_window(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    window.level = NSScreenSaverWindowLevel;
    window.hasShadow = NO;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary |
                                NSWindowCollectionBehaviorStationary;
}

// A panel that never becomes the key window: with the non-activating style, a click on it
// reaches its buttons without bringing the app to the front, so the app being recorded keeps
// the keyboard (measured with a probe app, docs/m5-results.md). An NSWindow can't be
// non-activating; the window becomes this panel class in place (no ivars of its own).
@interface KRFloatingPanel : NSPanel
@end
@implementation KRFloatingPanel
- (BOOL)canBecomeKeyWindow {
    return NO;
}
- (BOOL)canBecomeMainWindow {
    return NO;
}
@end

// The floating control and the camera bubble while recording: above normal windows and
// over full-screen apps, on every Space, no shadow (the page draws its own edge), and
// clicking it doesn't take the keyboard from the app being recorded.
static char kOriginalClass;

void kr_floating_window(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    // Kept to switch back before the window closes: the window library's own class must
    // handle that (closing it as the panel raised an exception, measured 2026-10-27).
    objc_setAssociatedObject(window, &kOriginalClass, object_getClass(window), OBJC_ASSOCIATION_ASSIGN);
    object_setClass(window, [KRFloatingPanel class]);
    window.styleMask |= NSWindowStyleMaskNonactivatingPanel;
    // After the class change, the style alone doesn't reach the window server's
    // "prevents activation" flag (as tauri-nspanel found); a click would still activate the
    // app. AppKit's own setter for it, where this macOS has one.
    SEL prevents = NSSelectorFromString(@"_setPreventsActivation:");
    if ([window respondsToSelector:prevents]) ((void (*)(id, SEL, BOOL))objc_msgSend)(window, prevents, YES);
    ((NSPanel *)window).floatingPanel = YES;
    ((NSPanel *)window).becomesKeyOnlyIfNeeded = YES;
    ((NSPanel *)window).hidesOnDeactivate = NO;
    window.level = NSStatusWindowLevel;
    window.hasShadow = NO;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;}

// Before a floating window closes: back to its own class and an ordinary activating window.
void kr_floating_window_release(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    Class original = objc_getAssociatedObject(window, &kOriginalClass);
    if (!original) return;
    SEL prevents = NSSelectorFromString(@"_setPreventsActivation:");
    if ([window respondsToSelector:prevents]) ((void (*)(id, SEL, BOOL))objc_msgSend)(window, prevents, NO);
    window.styleMask &= ~NSWindowStyleMaskNonactivatingPanel;
    object_setClass(window, original);
    objc_setAssociatedObject(window, &kOriginalClass, nil, OBJC_ASSOCIATION_ASSIGN);
}

// The window's number (its CGWindowID), for recording one of the app's own windows (the
// test version's pattern window).
uint32_t kr_window_number(void *ns_window) {
    NSWindow *window = (__bridge NSWindow *)ns_window;
    return window ? (uint32_t)window.windowNumber : 0;
}
