use std::ffi::c_void;

/// Status codes for the Nylon Ring ABI.
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NrStatus {
    Ok = 0,
    Err = 1,
    Invalid = 2,
    Unsupported = 3,
    /// Streaming completed normally.
    StreamEnd = 4,
}

/// A UTF-8 string slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrStr {
    pub ptr: *const u8,
    pub len: u32,
}

/// A byte slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrBytes {
    pub ptr: *const u8,
    pub len: u64,
}

/// A key-value pair of strings.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHeader {
    pub key: NrStr,
    pub value: NrStr,
}

/// Represents a request with metadata.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrRequest {
    pub path: NrStr,
    pub method: NrStr,
    pub query: NrStr,

    pub headers: *const NrHeader,
    pub headers_len: u32,

    // ABI forward-compatibility storage
    pub _reserved0: u32,
    pub _reserved1: u64,
}

/// Host callback table.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHostVTable {
    pub send_result:
        unsafe extern "C" fn(host_ctx: *mut c_void, sid: u64, status: NrStatus, payload: NrBytes),
}

/// Host extension table for state management.
/// This is an optional extension that does not modify the core ABI.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHostExt {
    /// Set state for a given sid and key.
    /// Returns empty NrBytes on success, or error bytes on failure.
    pub set_state: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
        value: NrBytes,
    ) -> NrBytes,

    /// Get state for a given sid and key.
    /// Returns empty NrBytes if not found.
    pub get_state: unsafe extern "C" fn(host_ctx: *mut c_void, sid: u64, key: NrStr) -> NrBytes,
}

// Safety: NrHostExt is ABI-stable data carrier.
unsafe impl Send for NrHostExt {}
unsafe impl Sync for NrHostExt {}

/// Plugin function table.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrPluginVTable {
    pub init: Option<
        unsafe extern "C" fn(
            plugin_ctx: *mut c_void,
            host_ctx: *mut c_void,
            host_vtable: *const NrHostVTable,
        ) -> NrStatus,
    >,

    pub handle: Option<
        unsafe extern "C" fn(
            plugin_ctx: *mut c_void,
            entry: NrStr,
            sid: u64,
            req: *const NrRequest,
            payload: NrBytes,
        ) -> NrStatus,
    >,

    pub handle_raw: Option<
        unsafe extern "C" fn(
            plugin_ctx: *mut c_void,
            entry: NrStr,
            sid: u64,
            payload: NrBytes,
        ) -> NrStatus,
    >,

    pub shutdown: Option<unsafe extern "C" fn(plugin_ctx: *mut c_void)>,
}

#[macro_export]
macro_rules! define_plugin {
    (
        init: $init_fn:path,
        shutdown: $shutdown_fn:path,
        entries: {
            $($entry_name:literal => $handler_fn:path),* $(,)?
        }
        $(, raw_entries: {
            $($raw_entry_name:literal => $raw_handler_fn:path),* $(,)?
        })?
    ) => {
        // Static VTable
        static PLUGIN_VTABLE: $crate::NrPluginVTable = $crate::NrPluginVTable {
            init: Some(plugin_init_wrapper),
            handle: Some(plugin_handle_wrapper),
            handle_raw: Some(plugin_handle_raw_wrapper),
            shutdown: Some(plugin_shutdown_wrapper),
        };

        // Static Plugin Info
        static PLUGIN_INFO: $crate::NrPluginInfo = $crate::NrPluginInfo {
            abi_version: 1,
            struct_size: std::mem::size_of::<$crate::NrPluginInfo>() as u32,
            name: $crate::NrStr {
                ptr: env!("CARGO_PKG_NAME").as_ptr(),
                len: env!("CARGO_PKG_NAME").len() as u32,
            },
            version: $crate::NrStr {
                ptr: env!("CARGO_PKG_VERSION").as_ptr(),
                len: env!("CARGO_PKG_VERSION").len() as u32,
            },
            plugin_ctx: std::ptr::null_mut(),
            vtable: &PLUGIN_VTABLE,
        };

        // Exported Entry Point
        #[unsafe(no_mangle)]
        pub extern "C" fn nylon_ring_get_plugin_v1() -> *const $crate::NrPluginInfo {
            &PLUGIN_INFO
        }

        // Wrappers
        unsafe extern "C" fn plugin_init_wrapper(
            plugin_ctx: *mut std::ffi::c_void,
            host_ctx: *mut std::ffi::c_void,
            host_vtable: *const $crate::NrHostVTable,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $init_fn(plugin_ctx, host_ctx, host_vtable)
            }));
            match result {
                Ok(status) => status,
                Err(_) => $crate::NrStatus::Err,
            }
        }

        unsafe extern "C" fn plugin_shutdown_wrapper(
            plugin_ctx: *mut std::ffi::c_void,
        ) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $shutdown_fn(plugin_ctx)
            }));
        }

        unsafe extern "C" fn plugin_handle_wrapper(
            plugin_ctx: *mut std::ffi::c_void,
            entry: $crate::NrStr,
            sid: u64,
            req: *const $crate::NrRequest,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let entry_str = entry.as_str();
                match entry_str {
                    $(
                        $entry_name => {
                            $handler_fn(plugin_ctx, sid, req, payload)
                        }
                    )*
                    _ => $crate::NrStatus::Invalid,
                }
            }));
            match result {
                Ok(status) => status,
                Err(_) => $crate::NrStatus::Err,
            }
        }

        unsafe extern "C" fn plugin_handle_raw_wrapper(
            plugin_ctx: *mut std::ffi::c_void,
            entry: $crate::NrStr,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let entry_str = entry.as_str();
                match entry_str {
                    $($(
                        $raw_entry_name => {
                            $raw_handler_fn(plugin_ctx, sid, payload)
                        }
                    )*)?
                    _ => $crate::NrStatus::Invalid,
                }
            }));
            match result {
                Ok(status) => status,
                Err(_) => $crate::NrStatus::Err,
            }
        }
    };
}

/// Metadata exported by the plugin.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrPluginInfo {
    pub abi_version: u32,
    pub struct_size: u32,

    pub name: NrStr,
    pub version: NrStr,

    pub plugin_ctx: *mut c_void,
    pub vtable: *const NrPluginVTable,
}

impl NrStr {
    pub fn from_str(s: &str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len() as u32,
        }
    }

    pub fn as_str(&self) -> &str {
        unsafe {
            let slice = std::slice::from_raw_parts(self.ptr, self.len as usize);
            std::str::from_utf8_unchecked(slice)
        }
    }
}

impl NrBytes {
    pub fn from_slice(s: &[u8]) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len() as u64,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
    }
}

impl NrHeader {
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            key: NrStr::from_str(key),
            value: NrStr::from_str(value),
        }
    }
}

impl NrPluginInfo {
    pub fn compatible(&self, expected_abi_version: u32) -> bool {
        self.abi_version == expected_abi_version
    }
}

// Safety: These types are ABI-stable data carriers.
// Users must ensure that the pointers they contain are valid and accessed safely.
unsafe impl Send for NrStr {}
unsafe impl Sync for NrStr {}

unsafe impl Send for NrBytes {}
unsafe impl Sync for NrBytes {}

unsafe impl Send for NrHeader {}
unsafe impl Sync for NrHeader {}

unsafe impl Send for NrRequest {}
unsafe impl Sync for NrRequest {}

unsafe impl Send for NrHostVTable {}
unsafe impl Sync for NrHostVTable {}

unsafe impl Send for NrPluginVTable {}
unsafe impl Sync for NrPluginVTable {}

unsafe impl Send for NrPluginInfo {}
unsafe impl Sync for NrPluginInfo {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    #[test]
    fn test_layout() {
        // Verify NrStr layout (ptr + u32)
        // On 64-bit: 8 bytes ptr + 4 bytes len + 4 bytes padding = 16 bytes
        assert_eq!(size_of::<NrStr>(), 16);
        assert_eq!(align_of::<NrStr>(), 8);

        // Verify NrBytes layout (ptr + u64)
        // On 64-bit: 8 bytes ptr + 8 bytes len = 16 bytes
        assert_eq!(size_of::<NrBytes>(), 16);
        assert_eq!(align_of::<NrBytes>(), 8);

        // Verify NrHeader layout (NrStr + NrStr)
        // 16 + 16 = 32 bytes
        assert_eq!(size_of::<NrHeader>(), 32);
        assert_eq!(align_of::<NrHeader>(), 8);

        // Verify NrRequest layout
        // path (16) + method (16) + query (16) + headers ptr (8) + headers_len (4) + reserved0 (4) + reserved1 (8)
        // 16*3 + 8 + 4 + 4 + 8 = 48 + 8 + 8 + 8 = 72 bytes
        assert_eq!(size_of::<NrRequest>(), 72);
        assert_eq!(align_of::<NrRequest>(), 8);
    }
}
