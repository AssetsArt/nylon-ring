//! ABI-facing types and plugin-definition helpers for Nylon Ring.
//!
//! Borrowed views such as [`NrStr`] and [`NrBytes`] do not carry Rust
//! lifetimes across the ABI boundary. Their accessors are therefore unsafe:
//! callers must guarantee that the referenced memory remains valid while the
//! returned borrow is used.

use std::ffi::c_void;
use std::fmt;

/// ABI version implemented by this crate.
pub const ABI_VERSION: u32 = 1;

/// Status code passed across the Nylon Ring ABI.
///
/// This is a transparent integer wrapper rather than a Rust enum so an
/// unknown value from a newer plugin remains well-defined. Values `7..=u32::MAX`
/// are reserved for future ABI versions.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct NrStatus(u32);

#[allow(non_upper_case_globals)]
impl NrStatus {
    pub const Ok: Self = Self(0);
    pub const Err: Self = Self(1);
    pub const Invalid: Self = Self(2);
    pub const Unsupported: Self = Self(3);
    /// Streaming completed normally.
    pub const StreamEnd: Self = Self(4);
    /// A plugin panic was contained at the FFI boundary.
    pub const Panic: Self = Self(5);
    /// A bounded stream queue cannot currently accept another frame.
    pub const Backpressure: Self = Self(6);

    /// Creates a status from its stable wire value.
    pub const fn from_raw(value: u32) -> Self {
        Self(value)
    }

    /// Returns the stable wire value.
    pub const fn as_raw(self) -> u32 {
        self.0
    }

    /// Returns whether this status terminates a response stream.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Err | Self::Invalid | Self::Unsupported | Self::StreamEnd | Self::Panic
        )
    }
}

impl fmt::Debug for NrStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Ok => "Ok",
            Self::Err => "Err",
            Self::Invalid => "Invalid",
            Self::Unsupported => "Unsupported",
            Self::StreamEnd => "StreamEnd",
            Self::Panic => "Panic",
            Self::Backpressure => "Backpressure",
            Self(value) => return f.debug_tuple("Unknown").field(&value).finish(),
        };
        f.write_str(name)
    }
}

/// A UTF-8 string slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
///
/// On 64-bit targets `_reserved` occupies the four bytes after `len` that
/// would otherwise be implicit padding. Producers must set it to zero.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct NrStr {
    pub ptr: *const u8,
    pub len: u32,
    pub _reserved: u32,
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

/// Error returned when an ABI string or byte view is malformed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NrViewError {
    /// A non-empty view has a null pointer.
    NullPointer,
    /// The ABI length cannot be represented by this platform's `usize`.
    LengthOverflow,
    /// A string view does not contain valid UTF-8.
    InvalidUtf8(std::str::Utf8Error),
}

impl fmt::Display for NrViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullPointer => f.write_str("non-empty ABI view has a null pointer"),
            Self::LengthOverflow => f.write_str("ABI view length exceeds usize"),
            Self::InvalidUtf8(error) => write!(f, "ABI string is not valid UTF-8: {error}"),
        }
    }
}

impl std::error::Error for NrViewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUtf8(error) => Some(error),
            Self::NullPointer | Self::LengthOverflow => None,
        }
    }
}

/// A key-value pair with any type as value.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Default)]
pub struct NrKVAny {
    key: NrStr,
    key_storage: NrVec<u8>,
    pub value: NrAny,
}

/// Index slot for hash table lookup.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct NrIndexSlot {
    pub hash: u64,
    pub entry_idx: u32, // index into entries
    pub state: u8,      // 0=empty, 1=full, 2=tombstone
    pub _pad: [u8; 3],
}

/// A map/dictionary type implemented as a vector of key-value pairs with hash index.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Default)]
pub struct NrMap {
    entries: NrVec<NrKVAny>,
    index: NrVec<NrIndexSlot>, // hash index table
    used: u32,                 // number of full slots
    tomb: u32,                 // number of tombstones
}

/// A type-erased value that can hold any data type.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug)]
pub struct NrAny {
    /// Pointer to the data
    data: *mut c_void,
    /// Size of the data in bytes
    size: u64,
    /// Type identifier (user-defined tag)
    type_tag: u32,
    /// Optional clone function pointer.
    clone_fn: Option<unsafe extern "C" fn(*const c_void) -> *mut c_void>,
    /// Optional destructor function pointer (can be null)
    drop_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// A vector with a pointer, length, and capacity.
/// This struct is `#[repr(C)]` and ABI-stable.
///
/// Owned values carry a producer-side drop callback so the receiving module
/// never deallocates them with the wrong allocator. Borrowed foreign values
/// use `owned = 0` and are never freed by this type. Prefer [`NrBytes`] for
/// ordinary borrowed input.
#[repr(C)]
#[derive(Debug)]
pub struct NrVec<T> {
    ptr: *mut T,
    len: usize,
    cap: usize,
    owned: u8,
    _reserved: [u8; 7],
    drop_fn: Option<unsafe extern "C" fn(*mut T, usize, usize)>,
}

impl<T> Default for NrVec<T> {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
            owned: 0,
            _reserved: [0; 7],
            drop_fn: None,
        }
    }
}

impl Default for NrAny {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            size: 0,
            type_tag: 0,
            clone_fn: None,
            drop_fn: None,
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
    pub send_result: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: NrVec<u8>,
    ) -> NrStatus,
}

/// Host extension table for state management.
/// This is an optional extension that does not modify the core ABI.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHostExt {
    /// Set state for a given sid and key.
    /// Returns a status code indicating whether the state was stored.
    pub set_state: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
        value: NrBytes,
    ) -> NrStatus,

    /// Get state for a given sid and key.
    /// Returns an owned, empty vector if the key is not found.
    pub get_state: unsafe extern "C" fn(host_ctx: *mut c_void, sid: u64, key: NrStr) -> NrVec<u8>,
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
            abi_version: $crate::ABI_VERSION,
            struct_size: std::mem::size_of::<$crate::NrPluginInfo>() as u32,
            name: $crate::NrStr::from_static(env!("CARGO_PKG_NAME")),
            version: $crate::NrStr::from_static(env!("CARGO_PKG_VERSION")),
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
            host_ctx: *mut std::ffi::c_void,
            host_vtable: *const $crate::NrHostVTable,
        ) -> $crate::NrStatus {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                $init_fn(host_ctx, host_vtable)
            }))
            .unwrap_or($crate::NrStatus::Panic)
        }

        unsafe extern "C" fn plugin_shutdown_wrapper() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $shutdown_fn();
            }));
        }

        unsafe extern "C" fn plugin_handle_wrapper(
            entry: $crate::NrStr,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let entry_str = match unsafe { entry.as_str() } {
                    Ok(entry) => entry,
                    Err(_) => return $crate::NrStatus::Invalid,
                };
                match entry_str {
                    $(
                        $entry_name => unsafe {
                            $handler_fn(sid, payload)
                        }
                    )*
                    _ => $crate::NrStatus::Invalid,
                }
            }))
            .unwrap_or($crate::NrStatus::Panic)
        }

        unsafe extern "C" fn plugin_stream_data_wrapper(
            sid: u64,
            data: $crate::NrBytes,
        ) -> $crate::NrStatus {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = (sid, data);
                $(
                    return unsafe { $stream_data_fn(sid, data) };
                )?
                #[allow(unreachable_code)]
                $crate::NrStatus::Unsupported
            }))
            .unwrap_or($crate::NrStatus::Panic)
        }

        unsafe extern "C" fn plugin_stream_close_wrapper(
            sid: u64,
        ) -> $crate::NrStatus {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = sid;
                $(
                    return unsafe { $stream_close_fn(sid) };
                )?
                #[allow(unreachable_code)]
                $crate::NrStatus::Unsupported
            }))
            .unwrap_or($crate::NrStatus::Panic)
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
    /// Creates a borrowed ABI string view.
    ///
    /// The returned value must not be used to access the string after `s` is
    /// invalidated. Use [`NrStr::as_str`] only while the source is alive.
    pub fn new(s: &str) -> Self {
        let len = u32::try_from(s.len()).expect("NrStr cannot represent strings larger than 4 GiB");
        Self {
            ptr: s.as_ptr(),
            len,
            _reserved: 0,
        }
    }

    /// Creates a borrowed ABI view from a static string.
    pub const fn from_static(s: &'static str) -> Self {
        assert!(
            s.len() <= u32::MAX as usize,
            "static string is too large for NrStr"
        );
        Self {
            ptr: s.as_ptr(),
            len: s.len() as u32,
            _reserved: 0,
        }
    }

    /// Reads this ABI view as UTF-8.
    ///
    /// # Safety
    ///
    /// For a non-empty view, `ptr` must be valid for reads of `len` bytes for
    /// the lifetime of the returned reference. The pointed-to memory must not
    /// be mutated while that reference exists.
    pub unsafe fn as_str<'a>(&self) -> Result<&'a str, NrViewError> {
        let bytes = unsafe { view_bytes(self.ptr, u64::from(self.len))? };
        std::str::from_utf8(bytes).map_err(NrViewError::InvalidUtf8)
    }

    /// Returns whether this view contains no bytes.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the view length in bytes.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns the underlying raw pointer.
    pub const fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Resets this view without freeing the borrowed memory.
    pub fn clear(&mut self) {
        self.ptr = std::ptr::null();
        self.len = 0;
        self._reserved = 0;
    }
}

unsafe fn view_bytes<'a>(ptr: *const u8, len: u64) -> Result<&'a [u8], NrViewError> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(NrViewError::NullPointer);
    }
    let len = usize::try_from(len).map_err(|_| NrViewError::LengthOverflow)?;
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

impl NrBytes {
    /// Creates a borrowed ABI byte view.
    pub fn from_slice(s: &[u8]) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: u64::try_from(s.len()).expect("NrBytes length does not fit in u64"),
        }
    }

    /// Reads this ABI byte view.
    ///
    /// # Safety
    ///
    /// For a non-empty view, `ptr` must be valid for reads of `len` bytes for
    /// the lifetime of the returned reference. The pointed-to memory must not
    /// be mutated while that reference exists.
    pub unsafe fn as_slice<'a>(&self) -> Result<&'a [u8], NrViewError> {
        unsafe { view_bytes(self.ptr, self.len) }
    }

    /// Returns whether this view contains no bytes.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the view length in bytes.
    pub const fn len(&self) -> u64 {
        self.len
    }
}

impl Clone for NrKVAny {
    fn clone(&self) -> Self {
        Self::new(self.key(), self.value.clone())
    }
}

impl Clone for NrMap {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            index: self.index.clone(),
            used: self.used,
            tomb: self.tomb,
        }
    }
}

impl Clone for NrAny {
    /// Clone an `NrAny`.
    ///
    /// # Safety / ABI v1 limitation
    ///
    /// `NrAny` is type-erased and ABI v1 has no `clone_fn` in the struct.
    /// Cloning a value that owns heap resources (i.e. has a `drop_fn`) by
    /// memcpy would shallow-copy any inner pointers and cause a double-free
    /// when both copies are dropped. Therefore:
    ///
    /// * Null payload  → returns `default()` (null).
    /// * `drop_fn = None` (POD payload) → safe deep copy via byte memcpy.
    /// * `drop_fn = Some(_)` → **panics**, because we cannot safely deep-copy
    ///   without a type-aware `clone_fn`. Use `drop_fn = None` and a POD
    ///   representation, or wait for ABI v2 which adds `clone_fn`.
    fn clone(&self) -> Self {
        if self.data.is_null() {
            return Self::default();
        }
        let clone_fn = self
            .clone_fn
            .expect("non-null NrAny values always have a clone function");
        let data = unsafe { clone_fn(self.data.cast_const()) };
        assert!(!data.is_null(), "NrAny clone callback failed");
        Self {
            data,
            size: self.size,
            type_tag: self.type_tag,
            clone_fn: self.clone_fn,
            drop_fn: self.drop_fn,
        }
    }
}

impl<T: Clone> Clone for NrVec<T> {
    fn clone(&self) -> Self {
        if self.len == 0 {
            return Self::default();
        }
        let v = self.as_slice().to_vec();
        Self::from_vec(v)
    }
}

impl NrKV {
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            key: NrStr::new(key),
            value: NrStr::new(value),
        }
    }

    pub fn from_nr_str(key: NrStr, value: NrStr) -> Self {
        Self { key, value }
    }
}

impl NrKVAny {
    pub fn new(key: &str, value: NrAny) -> Self {
        let key_storage = NrVec::from_vec(key.as_bytes().to_vec());
        let key = NrStr::new(
            std::str::from_utf8(key_storage.as_slice())
                .expect("key bytes originate from a valid UTF-8 string"),
        );
        Self {
            key,
            key_storage,
            value,
        }
    }

    /// Copies a borrowed ABI string into an owned key.
    ///
    /// # Safety
    ///
    /// `key` must point to readable memory for its declared length.
    pub unsafe fn from_nr_str(key: NrStr, value: NrAny) -> Result<Self, NrViewError> {
        let key = unsafe { key.as_str()? };
        Ok(Self::new(key, value))
    }

    /// Returns the owned key as a string.
    pub fn key(&self) -> &str {
        // The storage is copied from UTF-8 and cannot be mutated externally.
        unsafe { std::str::from_utf8_unchecked(self.key_storage.as_slice()) }
    }
}

// Hash function: FNV-1a
#[inline]
fn hash_str(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut h = FNV_OFFSET;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

impl NrMap {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn index_len(&self) -> usize {
        self.index.len
    }

    fn ensure_index(&mut self) {
        // Create index when we have enough entries (threshold = 8)
        if self.index.is_empty() && self.entries.len >= 8 {
            self.rehash(16);
        }
    }

    fn rehash(&mut self, mut new_cap: usize) {
        // Make it a power of 2 for fast masking
        new_cap = new_cap.next_power_of_two().max(16);

        // Create empty slots
        let mut slots = Vec::with_capacity(new_cap);
        slots.resize_with(new_cap, NrIndexSlot::default);

        self.index = NrVec::from_vec(slots);
        self.used = 0;
        self.tomb = 0;

        // Insert all entries into index
        for i in 0..self.entries.len {
            let kv = unsafe { &*self.entries.ptr.add(i) };
            let k = kv.key();
            let entry_idx =
                u32::try_from(i).expect("NrMap cannot contain more than u32::MAX entries");
            self.index_insert(hash_str(k), entry_idx);
        }
    }

    #[inline]
    fn should_grow(&self) -> bool {
        // Load factor approximately > 0.7 or too many tombstones
        if self.index.is_empty() {
            return false;
        }
        let occupied = u64::from(self.used) + u64::from(self.tomb);
        occupied * 10 >= self.index_len() as u64 * 7
    }

    fn maybe_grow(&mut self) {
        if self.should_grow() {
            let cap = self.index_len();
            self.rehash(cap * 2);
        }
    }

    fn index_insert(&mut self, hash: u64, entry_idx: u32) {
        let cap = self.index_len();
        if cap == 0 {
            return;
        }
        let mask = cap - 1;
        let mut pos = (hash as usize) & mask;
        let mut first_tomb: Option<usize> = None;

        for _ in 0..cap {
            let slot = unsafe { &mut *self.index.ptr.add(pos) };
            match slot.state {
                0 => {
                    let target = first_tomb.unwrap_or(pos);
                    let s2 = unsafe { &mut *self.index.ptr.add(target) };
                    s2.hash = hash;
                    s2.entry_idx = entry_idx;
                    s2.state = 1;
                    if first_tomb.is_some() {
                        self.tomb -= 1;
                    }
                    self.used += 1;
                    return;
                }
                2 if first_tomb.is_none() => first_tomb = Some(pos),
                _ => {}
            }
            pos = (pos + 1) & mask;
        }

        // Table is unexpectedly full -> rehash and try again
        let cap2 = cap * 2;
        self.rehash(cap2);
        self.index_insert(hash, entry_idx);
    }

    pub fn insert(&mut self, key: &str, value: NrAny) {
        // If key exists, replace the value (set behavior)
        if let Some(v) = self.get_mut(key) {
            *v = value;
            return;
        }

        assert!(
            self.entries.len < u32::MAX as usize,
            "NrMap cannot contain more than u32::MAX entries"
        );
        let kv = NrKVAny::new(key, value);
        self.entries.push(kv);

        self.ensure_index();
        if !self.index.is_empty() {
            self.maybe_grow();
            let idx = u32::try_from(self.entries.len - 1)
                .expect("NrMap cannot contain more than u32::MAX entries");
            self.index_insert(hash_str(key), idx);
        }
    }

    /// Copies an ABI string key and inserts the value.
    ///
    /// # Safety
    ///
    /// `key` must point to readable memory for its declared length.
    pub unsafe fn insert_nr(&mut self, key: NrStr, value: NrAny) -> Result<(), NrViewError> {
        let key_str = unsafe { key.as_str()? };
        // If key exists, replace the value (set behavior)
        if let Some(v) = self.get_mut(key_str) {
            *v = value;
            return Ok(());
        }

        assert!(
            self.entries.len < u32::MAX as usize,
            "NrMap cannot contain more than u32::MAX entries"
        );
        let kv = NrKVAny::new(key_str, value);
        self.entries.push(kv);

        self.ensure_index();
        if !self.index.is_empty() {
            self.maybe_grow();
            let idx = u32::try_from(self.entries.len - 1)
                .expect("NrMap cannot contain more than u32::MAX entries");
            self.index_insert(hash_str(key_str), idx);
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&NrAny> {
        if self.index.is_empty() {
            // Fallback to linear search (acceptable for small maps)
            for kv in self.entries.iter() {
                if kv.key() == key {
                    return Some(&kv.value);
                }
            }
            return None;
        }

        let h = hash_str(key);
        let cap = self.index.len;
        let mask = cap - 1;
        let mut pos = (h as usize) & mask;

        for _ in 0..cap {
            let slot = unsafe { &*self.index.ptr.add(pos) };
            match slot.state {
                0 => return None, // Empty slot found, key doesn't exist
                1 if slot.hash == h => {
                    let kv = unsafe { &*self.entries.ptr.add(slot.entry_idx as usize) };
                    if kv.key() == key {
                        return Some(&kv.value);
                    }
                }
                _ => {}
            }
            pos = (pos + 1) & mask;
        }
        None
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut NrAny> {
        if self.index.is_empty() {
            for kv in self.entries.iter_mut() {
                if kv.key() == key {
                    return Some(&mut kv.value);
                }
            }
            return None;
        }

        let h = hash_str(key);
        let cap = self.index.len;
        let mask = cap - 1;
        let mut pos = (h as usize) & mask;

        for _ in 0..cap {
            let slot = unsafe { &*self.index.ptr.add(pos) };
            match slot.state {
                0 => return None,
                1 if slot.hash == h => {
                    let kv = unsafe { &mut *self.entries.ptr.add(slot.entry_idx as usize) };
                    if kv.key() == key {
                        return Some(&mut kv.value);
                    }
                }
                _ => {}
            }
            pos = (pos + 1) & mask;
        }
        None
    }

    pub fn remove(&mut self, key: &str) -> Option<NrKVAny> {
        // Find the entry and its exact hash slot. Remembering the slot avoids
        // deleting the wrong entry when two keys have the same hash.
        let (idx, removed_slot) = if self.index.is_empty() {
            // Fallback to linear search
            (self.entries.iter().position(|kv| kv.key() == key)?, None)
        } else {
            // Use hash lookup
            let h = hash_str(key);
            let cap = self.index.len;
            let mask = cap - 1;
            let mut pos = (h as usize) & mask;
            let mut found: Option<(usize, usize)> = None;

            for _ in 0..cap {
                let slot = unsafe { &*self.index.ptr.add(pos) };
                match slot.state {
                    0 => break, // Empty slot found, key doesn't exist
                    1 if slot.hash == h => {
                        let entry_idx = slot.entry_idx as usize;
                        let kv = unsafe { &*self.entries.ptr.add(entry_idx) };
                        if kv.key() == key {
                            found = Some((entry_idx, pos));
                            break;
                        }
                    }
                    _ => {}
                }
                pos = (pos + 1) & mask;
            }

            let (entry_idx, slot) = found?;
            (entry_idx, Some(slot))
        };

        let last = self.entries.len - 1;

        // take removed
        let removed = unsafe { std::ptr::read(self.entries.ptr.add(idx)) };

        if idx != last {
            // Move last into idx (swap_remove)
            unsafe {
                let last_val = std::ptr::read(self.entries.ptr.add(last));
                std::ptr::write(self.entries.ptr.add(idx), last_val);
            }

            // Update index for the moved entry (last -> idx)
            if !self.index.is_empty() {
                let h_last = unsafe {
                    let kv = &*self.entries.ptr.add(idx);
                    hash_str(kv.key())
                };
                let cap = self.index.len;
                let mask = cap - 1;
                let mut pos = (h_last as usize) & mask;

                for _ in 0..cap {
                    let slot = unsafe { &mut *self.index.ptr.add(pos) };
                    if slot.state == 1 && slot.entry_idx == last as u32 {
                        slot.entry_idx = idx as u32;
                        break;
                    }
                    pos = (pos + 1) & mask;
                }
            }
        }

        self.entries.len -= 1;

        // Remove slot from index (mark as tombstone or rehash)
        if let Some(pos) = removed_slot {
            let slot = unsafe { &mut *self.index.ptr.add(pos) };
            debug_assert_eq!(slot.state, 1);
            slot.state = 2;
            self.used -= 1;
            self.tomb += 1;

            // Rehash if too many tombstones
            if self.should_grow() {
                self.rehash(self.index_len().max(16));
            }
        }

        Some(removed)
    }

    pub fn len(&self) -> usize {
        self.entries.len
    }

    pub fn is_empty(&self) -> bool {
        self.entries.len == 0
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index = NrVec::default();
        self.used = 0;
        self.tomb = 0;
    }
}

impl NrAny {
    /// Stores a clonable, thread-safe Rust value behind the ABI container.
    pub fn new<T: Clone + Send + Sync + 'static>(value: T, type_tag: u32) -> Self {
        let size = std::mem::size_of::<T>() as u64;
        let data = Box::into_raw(Box::new(value)) as *mut c_void;
        Self {
            data,
            size,
            type_tag,
            clone_fn: Some(clone_any::<T>),
            drop_fn: Some(drop_any::<T>),
        }
    }

    /// Stores an owned copy of a byte slice as a `Vec<u8>`.
    pub fn from_bytes(bytes: &[u8], type_tag: u32) -> Self {
        Self::new(bytes.to_vec(), type_tag)
    }

    /// Returns a typed raw pointer after checking the stored byte size.
    ///
    /// The user-defined type tag is not a Rust type identifier. The caller
    /// must verify [`NrAny::type_tag`] before dereferencing the returned pointer.
    pub fn as_ptr<T>(&self) -> Result<*const T, NrStatus> {
        if self.data.is_null() {
            return Err(NrStatus::Invalid);
        }
        let expected_size = std::mem::size_of::<T>() as u64;
        if self.size != expected_size {
            return Err(NrStatus::Err);
        }
        Ok(self.data as *const T)
    }

    pub fn as_mut_ptr<T>(&mut self) -> Result<*mut T, NrStatus> {
        if self.data.is_null() {
            return Err(NrStatus::Invalid);
        }
        let expected_size = std::mem::size_of::<T>() as u64;
        if self.size != expected_size {
            return Err(NrStatus::Err);
        }
        Ok(self.data as *mut T)
    }

    pub fn is_null(&self) -> bool {
        self.data.is_null()
    }

    pub fn type_tag(&self) -> u32 {
        self.type_tag
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

unsafe extern "C" fn clone_any<T: Clone>(ptr: *const c_void) -> *mut c_void {
    if !ptr.is_null() {
        return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let value = unsafe { &*ptr.cast::<T>() };
            Box::into_raw(Box::new(value.clone())).cast::<c_void>()
        }))
        .unwrap_or(std::ptr::null_mut());
    }
    std::ptr::null_mut()
}

unsafe extern "C" fn drop_any<T>(ptr: *mut c_void) {
    if !ptr.is_null() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(ptr.cast::<T>()));
        }));
    }
}

impl Drop for NrAny {
    fn drop(&mut self) {
        if let Some(drop_fn) = self.drop_fn
            && !self.data.is_null()
        {
            unsafe {
                drop_fn(self.data);
            }
        }
    }
}

impl NrPluginInfo {
    pub fn compatible(&self, expected_abi_version: u32) -> bool {
        self.abi_version == expected_abi_version
    }
}

impl NrVec<u8> {
    /// Copies an ABI byte view into an owned vector.
    ///
    /// # Safety
    ///
    /// `bytes` must point to readable memory for its declared length.
    pub unsafe fn from_nr_bytes(bytes: NrBytes) -> Result<Self, NrViewError> {
        let v = unsafe { bytes.as_slice()? }.to_vec();
        Ok(Self::from_vec(v))
    }

    pub fn from_string(s: String) -> Self {
        Self::from_vec(s.into_bytes())
    }
}

impl<T> NrVec<T> {
    /// Takes ownership of a Rust vector without copying its allocation.
    pub fn from_vec(v: Vec<T>) -> Self {
        let mut v = std::mem::ManuallyDrop::new(v);
        let ptr = v.as_mut_ptr();
        let len = v.len();
        let cap = v.capacity();
        Self {
            ptr,
            len,
            cap,
            owned: 1,
            _reserved: [0; 7],
            drop_fn: Some(drop_vec::<T>),
        }
    }

    /// Copies the elements into a vector owned by the current module.
    ///
    /// The source allocation is released by the allocator-specific callback
    /// supplied by the module that created this `NrVec`.
    pub fn into_vec(self) -> Vec<T>
    where
        T: Clone,
    {
        self.as_slice().to_vec()
    }

    fn push(&mut self, value: T) {
        if self.owned == 0 && self.ptr.is_null() && self.len == 0 && self.cap == 0 {
            self.owned = 1;
            self.drop_fn = Some(drop_vec::<T>);
        }
        assert_eq!(self.owned, 1, "cannot mutate a borrowed NrVec");
        if self.len == self.cap {
            self.reserve(1);
        }
        unsafe {
            std::ptr::write(self.ptr.add(self.len), value);
        }
        self.len += 1;
    }

    fn clear(&mut self) {
        assert_eq!(self.owned, 1, "cannot mutate a borrowed NrVec");
        while self.len > 0 {
            self.len -= 1;
            unsafe {
                std::ptr::drop_in_place(self.ptr.add(self.len));
            }
        }
    }

    fn reserve(&mut self, additional: usize) {
        assert_eq!(self.owned, 1, "cannot resize a borrowed NrVec");
        if std::mem::size_of::<T>() == 0 {
            assert!(
                self.len.checked_add(additional).is_some(),
                "capacity overflow"
            );
            if self.cap == 0 {
                self.ptr = std::ptr::NonNull::<T>::dangling().as_ptr();
                self.cap = usize::MAX;
            }
            return;
        }

        let available = self.cap - self.len;
        if available < additional {
            let required = self.len.checked_add(additional).expect("capacity overflow");
            let new_cap = if self.cap == 0 {
                std::cmp::max(1, required)
            } else {
                std::cmp::max(self.cap.saturating_mul(2), required)
            };

            let new_layout = match std::alloc::Layout::array::<T>(new_cap) {
                Ok(layout) => layout,
                Err(_) => {
                    // Layout calculation overflow - trigger allocation error
                    std::alloc::handle_alloc_error(
                        std::alloc::Layout::from_size_align(usize::MAX, 1)
                            .unwrap_or_else(|_| std::alloc::Layout::new::<u8>()),
                    )
                }
            };

            let new_ptr = if self.cap == 0 {
                unsafe { std::alloc::alloc(new_layout) }
            } else {
                let old_layout = match std::alloc::Layout::array::<T>(self.cap) {
                    Ok(layout) => layout,
                    Err(_) => {
                        // This should never happen since we successfully allocated before
                        // But handle it defensively
                        std::alloc::handle_alloc_error(new_layout)
                    }
                };
                unsafe { std::alloc::realloc(self.ptr as *mut u8, old_layout, new_layout.size()) }
            };

            if new_ptr.is_null() {
                std::alloc::handle_alloc_error(new_layout);
            }

            self.ptr = new_ptr as *mut T;
            self.cap = new_cap;
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr
    }
}

impl<T> Drop for NrVec<T> {
    fn drop(&mut self) {
        if self.owned == 1
            && let Some(drop_fn) = self.drop_fn
        {
            unsafe {
                drop_fn(self.ptr, self.len, self.cap);
            }
        }
    }
}

unsafe extern "C" fn drop_vec<T>(ptr: *mut T, len: usize, cap: usize) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if cap != 0 {
            unsafe {
                drop(Vec::from_raw_parts(ptr, len, cap));
            }
        }
    }));
}

impl<T> NrVec<T> {
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    pub fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
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

impl<T: Clone> IntoIterator for NrVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
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

unsafe impl Send for NrKVAny {}
unsafe impl Sync for NrKVAny {}

unsafe impl Send for NrMap {}
unsafe impl Sync for NrMap {}

unsafe impl Send for NrAny {}
unsafe impl Sync for NrAny {}

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

    unsafe fn test_plugin_init(_: *mut c_void, _: *const NrHostVTable) -> NrStatus {
        NrStatus::Ok
    }

    unsafe fn panicking_handler(_: u64, _: NrBytes) -> NrStatus {
        panic!("test panic must not cross the FFI boundary");
    }

    fn test_plugin_shutdown() {}

    define_plugin! {
        init: test_plugin_init,
        shutdown: test_plugin_shutdown,
        entries: {
            "panic" => panicking_handler,
        }
    }

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

        // ptr + len + cap + ownership/reserved + allocator-specific drop fn
        assert_eq!(size_of::<NrVec<u8>>(), 40);
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
    fn test_nr_vec_empty_and_zero_sized_values() {
        assert!(NrVec::<u8>::default().into_vec().is_empty());

        let mut values = NrVec::default();
        values.push(());
        values.push(());
        assert_eq!(values.len(), 2);
        assert_eq!(values.into_iter().count(), 2);
    }

    #[test]
    fn test_nr_vec_collect() {
        let mut v = NrVec::<u32>::default();
        v.push(10);
        v.push(20);

        let collected: Vec<u32> = v.iter().cloned().collect();
        assert_eq!(collected, vec![10, 20]);
    }

    #[test]
    fn test_nr_map() {
        let mut map = NrMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        // Insert string values
        let str_value1 = NrAny::new(String::from("value1"), 1);
        let str_value2 = NrAny::new(String::from("value2"), 1);
        map.insert("key1", str_value1);
        map.insert("key2", str_value2);
        assert_eq!(map.len(), 2);

        // Get and verify string values
        let value1 = map.get("key1").unwrap();
        let str_ptr1 = value1.as_ptr::<String>().unwrap();
        unsafe {
            assert_eq!(*str_ptr1, "value1");
        }

        let value2 = map.get("key2").unwrap();
        let str_ptr2 = value2.as_ptr::<String>().unwrap();
        unsafe {
            assert_eq!(*str_ptr2, "value2");
        }

        assert!(map.get("key3").is_none());

        // Test that get_mut returns a mutable reference
        let value_mut = map.get_mut("key1");
        assert!(value_mut.is_some());

        // Insert integer value
        let int_value = NrAny::new(42i32, 2);
        map.insert("key3", int_value);
        assert_eq!(map.len(), 3);

        let int_val = map.get("key3").unwrap();
        let int_ptr = int_val.as_ptr::<i32>().unwrap();
        unsafe {
            assert_eq!(*int_ptr, 42);
        }

        let removed = map.remove("key2");
        assert!(removed.is_some());
        assert_eq!(map.len(), 2);
        assert!(map.get("key2").is_none());

        map.clear();
        assert!(map.is_empty());

        // Keys are copied into the map and remain valid after the source dies.
        {
            let temporary_key = String::from("owned-key");
            map.insert(&temporary_key, NrAny::new(7_u32, 2));
        }
        let stored = map.get("owned-key").unwrap();
        assert_eq!(unsafe { *stored.as_ptr::<u32>().unwrap() }, 7);

        // Exercise the indexed path, then verify clear resets the index.
        for index in 0..16 {
            map.insert(&format!("key-{index}"), NrAny::new(index, 2));
        }
        for index in 0..16 {
            assert!(map.remove(&format!("key-{index}")).is_some());
        }
        map.clear();
        assert!(map.get("missing").is_none());
    }

    #[test]
    fn test_nr_any() {
        let any_int = NrAny::new(42i32, 1);
        assert!(!any_int.is_null());
        assert_eq!(any_int.type_tag(), 1);
        assert_eq!(any_int.size(), std::mem::size_of::<i32>() as u64);

        let ptr = any_int.as_ptr::<i32>().unwrap();
        unsafe {
            assert_eq!(*ptr, 42);
        }

        let any_string = NrAny::new(String::from("hello"), 2);
        assert_eq!(any_string.type_tag(), 2);
        let str_ptr = any_string.as_ptr::<String>().unwrap();
        unsafe {
            assert_eq!(*str_ptr, "hello");
        }
        let cloned_string = any_string.clone();
        let cloned_ptr = cloned_string.as_ptr::<String>().unwrap();
        unsafe {
            assert_eq!(*cloned_ptr, "hello");
            assert_ne!(str_ptr, cloned_ptr);
        }

        let any_bytes = NrAny::from_bytes(b"test", 3);
        assert_eq!(any_bytes.type_tag(), 3);
        assert_eq!(any_bytes.size(), std::mem::size_of::<Vec<u8>>() as u64);
        let bytes_ptr = any_bytes.as_ptr::<Vec<u8>>().unwrap();
        unsafe {
            assert_eq!(&*bytes_ptr, b"test");
        }

        let default_any = NrAny::default();
        assert!(default_any.is_null());
        assert_eq!(default_any.type_tag(), 0);
        assert_eq!(default_any.size(), 0);

        // Test null pointer error
        assert_eq!(default_any.as_ptr::<i32>(), Err(NrStatus::Invalid));
        let mut default_any_mut = NrAny::default();
        assert_eq!(default_any_mut.as_mut_ptr::<i32>(), Err(NrStatus::Invalid));

        // Test type mismatch error
        let any_int = NrAny::new(42i32, 1);
        assert_eq!(any_int.as_ptr::<u64>(), Err(NrStatus::Err)); // i32 size != u64 size
        let mut any_int_mut = NrAny::new(42i32, 1);
        assert_eq!(any_int_mut.as_mut_ptr::<u64>(), Err(NrStatus::Err));
    }

    #[test]
    fn test_borrowed_view_validation() {
        let empty = NrStr::default();
        assert_eq!(unsafe { empty.as_str() }.unwrap(), "");

        let invalid_utf8 = [0xff];
        let invalid = NrStr {
            ptr: invalid_utf8.as_ptr(),
            len: 1,
            _reserved: 0,
        };
        assert!(matches!(
            unsafe { invalid.as_str() },
            Err(NrViewError::InvalidUtf8(_))
        ));

        let null_bytes = NrBytes {
            ptr: std::ptr::null(),
            len: 1,
        };
        assert_eq!(
            unsafe { null_bytes.as_slice() },
            Err(NrViewError::NullPointer)
        );
    }

    #[test]
    fn plugin_macro_contains_panics_and_rejects_invalid_utf8() {
        let panic_entry = NrStr::new("panic");
        assert_eq!(
            unsafe { plugin_handle_wrapper(panic_entry, 1, NrBytes::default()) },
            NrStatus::Panic
        );

        let invalid_utf8 = [0xff];
        let invalid_entry = NrStr {
            ptr: invalid_utf8.as_ptr(),
            len: 1,
            _reserved: 0,
        };
        assert_eq!(
            unsafe { plugin_handle_wrapper(invalid_entry, 2, NrBytes::default()) },
            NrStatus::Invalid
        );
    }
}
