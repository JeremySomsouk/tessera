//! Sparkle owns download verification, installation, scheduling and preferences.
//! Plain cargo binaries and non-macOS builds do not start an updater.
#[cfg(target_os = "macos")]
mod native {
    use anyhow::{Context, bail};
    use objc2::{
        msg_send,
        rc::{Allocated, Retained},
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::{NSBundle, NSError, NSString, ns_string};

    #[derive(Default)]
    pub struct Updater {
        controller: Option<Retained<AnyObject>>,
        updater: Option<Retained<AnyObject>>,
        _framework: Option<Retained<NSBundle>>,
    }
    impl Updater {
        pub fn start() -> anyhow::Result<Self> {
            let bundle = NSBundle::mainBundle();
            if bundle
                .objectForInfoDictionaryKey(ns_string!("SUFeedURL"))
                .is_none()
            {
                return Ok(Self::default());
            }
            let _main_thread = objc2::MainThreadMarker::new()
                .context("Updates must start on the AppKit main thread")?;
            let path = std::path::PathBuf::from(bundle.bundlePath().to_string())
                .join("Contents/Frameworks/Sparkle.framework");
            let framework = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))
                .context("Sparkle framework is missing; reinstall Tessera")?;
            // Load only the framework shipped inside our signed application bundle.
            if !unsafe { framework.load() } {
                bail!("Cannot load Sparkle; reinstall Tessera");
            }
            let class = AnyClass::get(c"SPUStandardUpdaterController")
                .context("Sparkle updater class is unavailable")?;
            // Sparkle's documented Objective-C ABI. App::new is called on the AppKit
            // main thread. Both objects are retained for the application's lifetime.
            let controller: Retained<AnyObject> = unsafe {
                let allocated: Allocated<AnyObject> = msg_send![class, alloc];
                msg_send![allocated, initWithStartingUpdater: false,
                    updaterDelegate: None::<&AnyObject>, userDriverDelegate: None::<&AnyObject>]
            };
            let updater: Retained<AnyObject> = unsafe { msg_send![&controller, updater] };
            let mut error: Option<Retained<NSError>> = None;
            let started: bool = unsafe { msg_send![&updater, startUpdater: &mut error] };
            if !started {
                bail!(
                    "Cannot start updates: {}",
                    error
                        .map(|error| error.localizedDescription().to_string())
                        .unwrap_or_else(|| "unknown Sparkle error".into())
                );
            }
            let automatic: bool = unsafe { msg_send![&updater, automaticallyChecksForUpdates] };
            if automatic {
                // Only at startup, respecting Sparkle's persisted user preference.
                unsafe {
                    let _: () = msg_send![&updater, checkForUpdatesInBackground];
                }
            }
            Ok(Self {
                controller: Some(controller),
                updater: Some(updater),
                _framework: Some(framework),
            })
        }
        pub fn available(&self) -> bool {
            self.updater.is_some()
        }
        pub fn can_check(&self) -> bool {
            self.updater
                .as_ref()
                .is_some_and(|updater| unsafe { msg_send![updater, canCheckForUpdates] })
        }
        pub fn check(&self) {
            if self.can_check()
                && let Some(controller) = &self.controller
            {
                unsafe {
                    let _: () = msg_send![controller, checkForUpdates: None::<&AnyObject>];
                }
            }
        }
        pub fn automatic_checks(&self) -> bool {
            self.updater
                .as_ref()
                .is_some_and(|updater| unsafe { msg_send![updater, automaticallyChecksForUpdates] })
        }
        pub fn set_automatic_checks(&self, enabled: bool) {
            if let Some(updater) = &self.updater {
                unsafe {
                    let _: () = msg_send![updater, setAutomaticallyChecksForUpdates: enabled];
                }
            }
        }
        pub fn automatic_downloads(&self) -> bool {
            self.updater
                .as_ref()
                .is_some_and(|updater| unsafe { msg_send![updater, automaticallyDownloadsUpdates] })
        }
        pub fn set_automatic_downloads(&self, enabled: bool) {
            if let Some(updater) = &self.updater {
                unsafe {
                    let _: () = msg_send![updater, setAutomaticallyDownloadsUpdates: enabled];
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use native::Updater;

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct Updater {}
#[cfg(not(target_os = "macos"))]
impl Updater {
    pub fn start() -> anyhow::Result<Self> {
        Ok(Self {})
    }
    pub fn available(&self) -> bool {
        false
    }
    pub fn can_check(&self) -> bool {
        false
    }
    pub fn check(&self) {}
    pub fn automatic_checks(&self) -> bool {
        false
    }
    pub fn set_automatic_checks(&self, _enabled: bool) {}
    pub fn automatic_downloads(&self) -> bool {
        false
    }
    pub fn set_automatic_downloads(&self, _enabled: bool) {}
}
