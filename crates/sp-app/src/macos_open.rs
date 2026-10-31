//! Finder/Open-With document delivery for the macOS app bundle.
//!
//! Finder delivers documents as an `aevt/odoc` Apple Event. Two hooks catch
//! it, because a single one cannot cover both launch modes:
//!
//! * **Cold launch** (double-click a RAW while ShopPhoto is not running).
//!   NSApplication dispatches the launch-time `odoc` from inside
//!   `finishLaunching`, *before* `applicationDidFinishLaunching:` and thus
//!   before winit/eframe construct the app. Its built-in handler forwards to
//!   the delegate's `application:openURLs:` if that exists, and otherwise
//!   shows Finder's "ShopPhoto cannot open files in the … format" alert.
//!   [`install_delegate_hooks`] adds that method to winit's delegate class
//!   right after the event loop is built, so the file is queued before the
//!   UI exists.
//! * **Warm launch** (file opened while ShopPhoto is already running). The
//!   delegate route also works here; [`install_handler`] additionally
//!   registers a direct Apple Event handler from app construction as a
//!   fallback for winit versions that ship their own delegate method.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSApplication;
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSObject, NSObjectProtocol, NSString,
    NSURL,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS_EVENT: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT_KEY: u32 = u32::from_be_bytes(*b"----");
const FILE_URL_DESCRIPTOR: u32 = u32::from_be_bytes(*b"furl");

static OPEN_FILES: OnceLock<Mutex<VecDeque<PathBuf>>> = OnceLock::new();
static EGUI_CONTEXT: OnceLock<egui::Context> = OnceLock::new();
static INSTALLED: OnceLock<()> = OnceLock::new();

fn enqueue_url(url: &NSURL) {
    if !url.isFileURL() {
        return;
    }
    let Some(path) = url.path() else {
        return;
    };
    enqueue_path(PathBuf::from(path.to_string()));
}

fn enqueue_path(path: PathBuf) {
    if !path.is_file() || !sp_raw::is_supported_image(&path) {
        return;
    }
    let mut queue = OPEN_FILES
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Both delivery routes may observe the same event; never open it twice.
    if queue.back() == Some(&path) {
        return;
    }
    tracing::info!(path = %path.display(), "queued file opened from Finder");
    queue.push_back(path);
}

fn wake_ui() {
    if let Some(ctx) = EGUI_CONTEXT.get() {
        ctx.request_repaint();
    }
}

/// `- (void)application:(NSApplication *)app openURLs:(NSArray<NSURL *> *)urls`
extern "C" fn application_open_urls(
    _this: &AnyObject,
    _cmd: Sel,
    _app: &AnyObject,
    urls: &NSArray<NSURL>,
) {
    for url in urls {
        enqueue_url(&url);
    }
    wake_ui();
}

/// `- (void)application:(NSApplication *)app openFiles:(NSArray<NSString *> *)files`
extern "C" fn application_open_files(
    _this: &AnyObject,
    _cmd: Sel,
    _app: &AnyObject,
    files: &NSArray<NSString>,
) {
    for file in files {
        enqueue_path(PathBuf::from(file.to_string()));
    }
    wake_ui();
}

/// Teach winit's `NSApplicationDelegate` to accept documents. Must run after
/// the winit event loop is built (which installs that delegate) and before
/// it runs, so the launch-time Open Documents event finds the method.
pub fn install_delegate_hooks() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let delegate: *mut AnyObject = unsafe { msg_send![&app, delegate] };
    if delegate.is_null() {
        tracing::warn!("no NSApplication delegate; Finder documents will only open while running");
        return;
    }
    // SAFETY: `delegate` is a live object owned by NSApplication; the class
    // pointer it returns is immortal.
    let class = unsafe { objc2::ffi::object_getClass(delegate) }.cast_mut();
    let hooks: [(&std::ffi::CStr, Imp); 2] = [
        (c"application:openURLs:", unsafe {
            std::mem::transmute::<extern "C" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>), Imp>(
                application_open_urls,
            )
        }),
        (c"application:openFiles:", unsafe {
            std::mem::transmute::<extern "C" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSString>), Imp>(
                application_open_files,
            )
        }),
    ];
    for (name, imp) in hooks {
        let selector = Sel::register(name);
        // SAFETY: adding an instance method with the standard
        // `v@:@@` signature to a class that does not define it yet.
        let added = unsafe {
            if objc2::ffi::class_respondsToSelector(class, selector).as_bool() {
                // winit now handles this itself; leave its implementation.
                continue;
            }
            objc2::ffi::class_addMethod(class, selector, imp, c"v@:@@".as_ptr())
        };
        if !added.as_bool() {
            tracing::warn!(selector = ?name, "could not add Finder open hook to app delegate");
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct OpenDocumentsHandler;

    unsafe impl NSObjectProtocol for OpenDocumentsHandler {}

    impl OpenDocumentsHandler {
        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle_open_documents(
            &self,
            event: &NSAppleEventDescriptor,
            _reply: &NSAppleEventDescriptor,
        ) {
            let Some(files) = event.paramDescriptorForKeyword(DIRECT_OBJECT_KEY) else {
                return;
            };
            // Apple Event descriptor lists use one-based indexes.
            for index in 1..=files.numberOfItems() {
                let Some(descriptor) = files.descriptorAtIndex(index) else {
                    continue;
                };
                // Finder normally sends alias descriptors, not NSURL
                // descriptors. Coerce aliases to `typeFileURL` explicitly;
                // fileURLValue() alone returns None for those.
                let url = descriptor.fileURLValue().or_else(|| {
                    descriptor
                        .coerceToDescriptorType(FILE_URL_DESCRIPTOR)
                        .and_then(|file_url| file_url.fileURLValue())
                });
                if let Some(url) = url {
                    enqueue_url(&url);
                } else {
                    tracing::warn!(
                        descriptor_type = descriptor.descriptorType(),
                        "Finder Open Documents event contained an unreadable file descriptor"
                    );
                }
            }
            wake_ui();
        }
    }
);

impl OpenDocumentsHandler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(mtm.alloc().set_ivars(())), init] }
    }
}

pub fn set_context(ctx: &egui::Context) {
    let _ = EGUI_CONTEXT.set(ctx.clone());
    // NSApplication installs its own Open Documents handler while finishing
    // launch. Register ours from app construction (inside that launch
    // callback), after the default handler exists, so it remains active for
    // later Finder events.
    install_handler();
}

/// Register directly for the `aevt/odoc` Apple Event during application
/// construction. This works for both cold launches and files opened while
/// ShopPhoto is already running, and prevents NSApplication's default
/// "cannot open RAW Photo" response.
pub fn install_handler() {
    if INSTALLED.get().is_some() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let handler = OpenDocumentsHandler::new(mtm);
    let manager = NSAppleEventManager::sharedAppleEventManager();
    unsafe {
        manager.setEventHandler_andSelector_forEventClass_andEventID(
            &handler,
            sel!(handleOpenDocuments:withReplyEvent:),
            CORE_EVENT_CLASS,
            OPEN_DOCUMENTS_EVENT,
        );
    }
    // NSAppleEventManager does not own its target. Keep this process-sized
    // handler alive; it contains no resources beyond the NSObject header.
    std::mem::forget(handler);
    let _ = INSTALLED.set(());
}

pub fn take_open_file() -> Option<PathBuf> {
    OPEN_FILES
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop_front()
}
