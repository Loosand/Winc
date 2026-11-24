//! Public Foundation and Darwin metadata for iCloud Drive. No file contents are opened here.

use super::files::file_url;
use super::*;
use crate::app::workspace::cloud::CloudStatus;
use objc2::rc::autoreleasepool;
use std::os::macos::fs::MetadataExt;

// sys/stat.h. File Provider placeholders on current macOS may only set this flag.
const SF_DATALESS: u32 = 0x4000_0000;

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSURLIsUbiquitousItemKey: &'static AnyObject;
    static NSURLUbiquitousItemIsDownloadingKey: &'static AnyObject;
    static NSURLUbiquitousItemIsUploadingKey: &'static AnyObject;
    static NSURLUbiquitousItemDownloadingStatusKey: &'static AnyObject;
    static NSURLUbiquitousItemDownloadingStatusNotDownloaded: &'static AnyObject;
    static NSURLUbiquitousItemDownloadingErrorKey: &'static AnyObject;
    static NSURLUbiquitousItemUploadingErrorKey: &'static AnyObject;
}

pub(crate) fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    metadata.st_flags() & SF_DATALESS != 0
}

pub(crate) fn is_icloud_item(path: &Path) -> bool {
    autoreleasepool(|_| {
        let Some(url) = file_url(path, true) else {
            return false;
        };
        let mut value: *mut AnyObject = std::ptr::null_mut();
        // SAFETY: Foundation URL resource values can be queried on a worker. The key is a
        // Foundation NSString constant and the returned NSNumber may be nil.
        unsafe {
            let ok: bool = msg_send![&*url, getResourceValue: &mut value,
                forKey: NSURLIsUbiquitousItemKey, error: std::ptr::null_mut::<*mut AnyObject>()];
            ok && value
                .as_ref()
                .is_some_and(|value| msg_send![value, boolValue])
        }
    })
}

fn status(
    failed: bool,
    downloading: bool,
    uploading: bool,
    online_only: bool,
) -> Option<CloudStatus> {
    if failed {
        Some(CloudStatus::Failed)
    } else if downloading {
        Some(CloudStatus::Downloading)
    } else if uploading {
        Some(CloudStatus::Uploading)
    } else if online_only {
        Some(CloudStatus::OnlineOnly)
    } else {
        None
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "iCloud Drive status uses local filesystem metadata"
)]
pub(crate) fn icloud_file_status(path: &Path) -> Option<CloudStatus> {
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let dataless = is_dataless(&metadata);
    autoreleasepool(|_| {
        let Some(url) = file_url(path, false) else {
            return status(false, false, false, dataless);
        };
        // SAFETY: NSArray copies the pointers to the live Foundation keys. NSURL resource
        // values, the dictionary and its documented NSNumber/NSString values stay alive for
        // the autorelease pool. The APIs are safe to call from this background worker.
        unsafe {
            let keys: [*const AnyObject; 5] = [
                NSURLUbiquitousItemIsDownloadingKey,
                NSURLUbiquitousItemIsUploadingKey,
                NSURLUbiquitousItemDownloadingStatusKey,
                NSURLUbiquitousItemDownloadingErrorKey,
                NSURLUbiquitousItemUploadingErrorKey,
            ];
            let array: *mut AnyObject =
                msg_send![class!(NSArray), arrayWithObjects: keys.as_ptr(), count: keys.len()];
            let values: *mut AnyObject = msg_send![&*url, resourceValuesForKeys: array,
                error: std::ptr::null_mut::<*mut AnyObject>()];
            let Some(values) = values.as_ref() else {
                return status(false, false, false, dataless);
            };
            let get = |key: &AnyObject| {
                let value: *mut AnyObject = msg_send![values, objectForKey: key];
                value.as_ref()
            };
            let failed = get(NSURLUbiquitousItemDownloadingErrorKey).is_some()
                || get(NSURLUbiquitousItemUploadingErrorKey).is_some();
            let downloading = get(NSURLUbiquitousItemIsDownloadingKey)
                .is_some_and(|value| msg_send![value, boolValue]);
            let uploading = get(NSURLUbiquitousItemIsUploadingKey)
                .is_some_and(|value| msg_send![value, boolValue]);
            let online_only = dataless || get(NSURLUbiquitousItemDownloadingStatusKey)
                .is_some_and(|value| msg_send![value, isEqualToString: NSURLUbiquitousItemDownloadingStatusNotDownloaded]);
            status(failed, downloading, uploading, online_only)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_active_transfer_or_error_takes_precedence_over_a_placeholder() {
        assert_eq!(
            status(false, false, false, true),
            Some(CloudStatus::OnlineOnly)
        );
        assert_eq!(
            status(false, true, false, true),
            Some(CloudStatus::Downloading)
        );
        assert_eq!(
            status(false, false, true, false),
            Some(CloudStatus::Uploading)
        );
        assert_eq!(status(true, true, false, true), Some(CloudStatus::Failed));
        assert_eq!(status(false, false, false, false), None);
    }

    #[test]
    fn regular_local_file_has_no_cloud_badge() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(!is_icloud_item(file.parent().unwrap()));
        assert_eq!(icloud_file_status(&file), None);
    }
}
