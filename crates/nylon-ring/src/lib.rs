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
#[derive(Debug, Copy, Clone, Default)]
pub struct NrStr {
    pub ptr: *const u8,
    pub len: u32,
}

/// A byte slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct NrBytes {
    pub ptr: *const u8,
    pub len: u64,
}

/// A key-value pair of strings.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct NrKV {
    pub key: NrStr,
    pub value: NrStr,
}

/// A vector with a pointer, length, and capacity.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NrVec<T> {
    pub ptr: *mut T,
    pub len: u64,
    pub cap: u64,
}

impl<T> Default for NrVec<T> {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }
}

/// A tuple of two elements.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NrTuple<A, B> {
    pub a: A,
    pub b: B,
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
        unsafe extern "C" fn(host_ctx: *mut c_void, host_vtable: *const NrHostVTable) -> NrStatus,
    >,

    pub handle: Option<unsafe extern "C" fn(entry: NrStr, sid: u64, payload: NrBytes) -> NrStatus>,

    pub shutdown: Option<unsafe extern "C" fn()>,

    pub stream_data: Option<unsafe extern "C" fn(sid: u64, data: NrBytes) -> NrStatus>,

    pub stream_close: Option<unsafe extern "C" fn(sid: u64) -> NrStatus>,
}

#[macro_export]
macro_rules! define_plugin {
    (
        init: $init_fn:path,
        shutdown: $shutdown_fn:path,
        entries: {
            $($entry_name:literal => $handler_fn:path),* $(,)?
        }
        $(, stream_handlers: {
            data: $stream_data_fn:path,
            close: $stream_close_fn:path $(,)?
        })?
    ) => {
        // Static VTable
        static PLUGIN_VTABLE: $crate::NrPluginVTable = $crate::NrPluginVTable {
            init: Some(plugin_init_wrapper),
            handle: Some(plugin_handle_wrapper),
            shutdown: Some(plugin_shutdown_wrapper),
            stream_data: Some(plugin_stream_data_wrapper),
            stream_close: Some(plugin_stream_close_wrapper),
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

        unsafe extern "C" fn plugin_shutdown_wrapper() {
            $shutdown_fn();
        }

        unsafe extern "C" fn plugin_handle_wrapper(
            entry: $crate::NrStr,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let entry_str = entry.as_str();
                match entry_str {
                    $($(
                        $entry_name => {
                            $handler_fn(sid, payload)
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

        unsafe extern "C" fn plugin_stream_data_wrapper(
            sid: u64,
            data: $crate::NrBytes,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $(
                    return $stream_data_fn(sid, data);
                )?
                #[allow(unreachable_code)]
                $crate::NrStatus::Unsupported
            }));
            match result {
                Ok(status) => status,
                Err(_) => $crate::NrStatus::Err,
            }
        }

        unsafe extern "C" fn plugin_stream_close_wrapper(
            sid: u64,
        ) -> $crate::NrStatus {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $(
                    return $stream_close_fn(sid);
                )?
                #[allow(unreachable_code)]
                $crate::NrStatus::Unsupported
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

    // push_str
    pub fn push_str(&mut self, s: &str) {
        if self.ptr.is_null() {
            self.ptr = s.as_ptr();
            self.len = s.len() as u32;
            return;
        }
        let new_len = self.len + s.len() as u32;
        let new_slice =
            unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u8, new_len as usize) };
        new_slice[self.len as usize..new_len as usize].copy_from_slice(s.as_bytes());
        self.len = new_len;
    }

    pub fn clear(&mut self) {
        self.ptr = std::ptr::null();
        self.len = 0;
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

impl NrKV {
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            key: NrStr::from_str(key),
            value: NrStr::from_str(value),
        }
    }

    pub fn from_nr_str(key: NrStr, value: NrStr) -> Self {
        Self { key, value }
    }
}

impl NrPluginInfo {
    pub fn compatible(&self, expected_abi_version: u32) -> bool {
        self.abi_version == expected_abi_version
    }
}

impl<T> NrVec<T> {
    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.reserve(1);
        }
        unsafe {
            std::ptr::write(self.ptr.add(self.len as usize), value);
        }
        self.len += 1;
    }

    pub fn clear(&mut self) {
        while self.len > 0 {
            self.len -= 1;
            unsafe {
                std::ptr::drop_in_place(self.ptr.add(self.len as usize));
            }
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        let available = self.cap as usize - self.len as usize;
        if available < additional {
            let required = self.len as usize + additional;
            let new_cap = if self.cap == 0 {
                std::cmp::max(1, required)
            } else {
                std::cmp::max(self.cap as usize * 2, required)
            };

            let new_layout = std::alloc::Layout::array::<T>(new_cap).unwrap();

            let new_ptr = if self.cap == 0 {
                unsafe { std::alloc::alloc(new_layout) }
            } else {
                let old_layout = std::alloc::Layout::array::<T>(self.cap as usize).unwrap();
                unsafe { std::alloc::realloc(self.ptr as *mut u8, old_layout, new_layout.size()) }
            };

            if new_ptr.is_null() {
                std::alloc::handle_alloc_error(new_layout);
            }

            self.ptr = new_ptr as *mut T;
            self.cap = new_cap as u64;
        }
    }
}

impl<T> Drop for NrVec<T> {
    fn drop(&mut self) {
        if self.cap != 0 {
            if self.ptr.is_null() {
                return;
            }
            unsafe {
                // Drop elements
                let s = std::slice::from_raw_parts_mut(self.ptr, self.len as usize);
                std::ptr::drop_in_place(s);

                // Deallocate
                if let Ok(layout) = std::alloc::Layout::array::<T>(self.cap as usize) {
                    std::alloc::dealloc(self.ptr as *mut u8, layout);
                }
            }
        }
    }
}

impl<T> NrVec<T> {
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    pub fn as_slice(&self) -> &[T] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.ptr.is_null() {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len as usize) }
        }
    }
}

impl<'a, T> IntoIterator for &'a NrVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut NrVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// An iterator that moves out of an NrVec.
pub struct IntoIter<T> {
    buf: *mut T,
    cap: usize,
    ptr: *const T,
    end: *const T,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                let result = std::ptr::read(self.ptr);
                self.ptr = self.ptr.add(1);
                Some(result)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = (self.end as usize - self.ptr as usize) / std::mem::size_of::<T>();
        (len, Some(len))
    }
}

impl<T> Drop for IntoIter<T> {
    fn drop(&mut self) {
        // Drop remaining elements
        if self.ptr != self.end {
            unsafe {
                let len = (self.end as usize - self.ptr as usize) / std::mem::size_of::<T>();
                let s = std::slice::from_raw_parts_mut(self.ptr as *mut T, len);
                std::ptr::drop_in_place(s);
            }
        }
        // Deallocate buffer
        if self.cap != 0 {
            unsafe {
                if let Ok(layout) = std::alloc::Layout::array::<T>(self.cap) {
                    std::alloc::dealloc(self.buf as *mut u8, layout);
                }
            }
        }
    }
}

impl<T> IntoIterator for NrVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        // Prevent NrVec drop from deallocating
        let this = std::mem::ManuallyDrop::new(self);

        let ptr = this.ptr;
        let cap = this.cap as usize;
        let len = this.len as usize;

        unsafe {
            IntoIter {
                buf: ptr,
                cap,
                ptr,
                end: if ptr.is_null() { ptr } else { ptr.add(len) },
            }
        }
    }
}

// Safety: These types are ABI-stable data carriers.
// Users must ensure that the pointers they contain are valid and accessed safely.
unsafe impl Send for NrStr {}
unsafe impl Sync for NrStr {}

unsafe impl Send for NrBytes {}
unsafe impl Sync for NrBytes {}

unsafe impl Send for NrKV {}
unsafe impl Sync for NrKV {}

unsafe impl Send for NrHostVTable {}
unsafe impl Sync for NrHostVTable {}

unsafe impl Send for NrPluginVTable {}
unsafe impl Sync for NrPluginVTable {}

unsafe impl Send for NrPluginInfo {}
unsafe impl Sync for NrPluginInfo {}

unsafe impl<T: Send> Send for NrVec<T> {}
unsafe impl<T: Sync> Sync for NrVec<T> {}

unsafe impl<A: Send, B: Send> Send for NrTuple<A, B> {}
unsafe impl<A: Sync, B: Sync> Sync for NrTuple<A, B> {}

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

        // Verify NrVec layout (ptr + u64 + u64)
        // On 64-bit: 8 bytes ptr + 8 bytes len + 8 bytes cap = 24 bytes
        assert_eq!(size_of::<NrVec<u8>>(), 24);
        assert_eq!(align_of::<NrVec<u8>>(), 8);

        // Verify NrTuple layout (A + B)
        // u64 + u64 = 16 bytes
        assert_eq!(size_of::<NrTuple<u64, u64>>(), 16);
        assert_eq!(align_of::<NrTuple<u64, u64>>(), 8);

        // Verify NrKV layout (NrStr + NrStr)
        // 16 + 16 = 32 bytes
        assert_eq!(size_of::<NrKV>(), 32);
        assert_eq!(align_of::<NrKV>(), 8);
    }

    #[test]
    fn test_nr_vec() {
        let mut v = NrVec::<u32>::default();
        assert_eq!(v.len, 0);
        assert_eq!(v.cap, 0);

        v.push(1);
        assert_eq!(v.len, 1);
        assert!(v.cap >= 1);
        unsafe {
            assert_eq!(*v.ptr, 1);
        }

        v.push(2);
        assert_eq!(v.len, 2);
        unsafe {
            assert_eq!(*v.ptr.add(1), 2);
        }

        v.reserve(10);
        assert!(v.cap >= 12); // 2 + 10

        v.clear();
        assert_eq!(v.len, 0);
        assert!(v.cap >= 12);
    }
    #[test]
    fn test_nr_vec_iter() {
        let mut v = NrVec::<u32>::default();
        v.push(1);
        v.push(2);
        v.push(3);

        let mut iter = v.iter();
        assert_eq!(iter.next(), Some(&1));
        assert_eq!(iter.next(), Some(&2));
        assert_eq!(iter.next(), Some(&3));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_nr_vec_iter_mut() {
        let mut v = NrVec::<u32>::default();
        v.push(1);
        v.push(2);
        v.push(3);

        for x in v.iter_mut() {
            *x *= 2;
        }

        let mut iter = v.iter();
        assert_eq!(iter.next(), Some(&2));
        assert_eq!(iter.next(), Some(&4));
        assert_eq!(iter.next(), Some(&6));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_nr_vec_into_iter() {
        let mut v = NrVec::<u32>::default();
        v.push(1);
        v.push(2);
        v.push(3);

        let mut iter = v.into_iter();
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_nr_vec_collect() {
        let mut v = NrVec::<u32>::default();
        v.push(10);
        v.push(20);

        let collected: Vec<u32> = v.iter().cloned().collect();
        assert_eq!(collected, vec![10, 20]);
    }
}
