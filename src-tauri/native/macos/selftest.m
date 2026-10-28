// For `rec --self-test` (src/selftest.rs): throws the kind of Objective-C exception WebKit
// throws when an answer reaches a request it already stopped, so the self-test can check
// that the release binary catches it as wry does (objc2::exception::catch). With the
// release profile at panic = "abort" this aborted the app (v0.1.0-alpha draft, 2026-10-28).

#import <Foundation/Foundation.h>

void kr_selftest_throw(void) {
    @throw [NSException exceptionWithName:NSInternalInconsistencyException
                                   reason:@"self-test: this task has already been stopped"
                                 userInfo:nil];
}
