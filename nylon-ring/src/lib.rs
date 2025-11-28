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
            sid: u64,
            req: *const NrRequest,
            payload: NrBytes,
        ) -> NrStatus,
    >,

    pub shutdown: Option<unsafe extern "C" fn(plugin_ctx: *mut c_void)>,
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
