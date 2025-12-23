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

/// A key-value pair with any type as value.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NrKVAny {
    pub key: NrStr,
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
#[derive(Debug, Clone)]
pub struct NrMap {
    pub entries: NrVec<NrKVAny>,
    pub index: NrVec<NrIndexSlot>, // hash index table
    pub used: u32,                 // number of full slots
    pub tomb: u32,                 // number of tombstones
}

/// A type-erased value that can hold any data type.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NrAny {
    /// Pointer to the data
    pub data: *mut c_void,
    /// Size of the data in bytes
    pub size: u64,
    /// Type identifier (user-defined tag)
    pub type_tag: u32,
    /// Optional destructor function pointer (can be null)
    pub drop_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// A vector with a pointer, length, and capacity.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NrVec<T> {
    pub ptr: *mut T,
    pub len: usize,
    pub cap: usize,
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

impl Default for NrKVAny {
    fn default() -> Self {
        Self {
            key: NrStr::default(),
            value: NrAny::default(),
        }
    }
}

impl Default for NrMap {
    fn default() -> Self {
        Self {
            entries: NrVec::default(),
            index: NrVec::default(),
            used: 0,
            tomb: 0,
        }
    }
}

impl Default for NrAny {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            size: 0,
            type_tag: 0,
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
    pub send_result:
        unsafe extern "C" fn(host_ctx: *mut c_void, sid: u64, status: NrStatus, payload: NrVec<u8>),
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
            host_ctx: *mut std::ffi::c_void,
            host_vtable: *const $crate::NrHostVTable,
        ) -> $crate::NrStatus {
            $init_fn(host_ctx, host_vtable)
        }

        unsafe extern "C" fn plugin_shutdown_wrapper() {
            $shutdown_fn();
        }

        unsafe extern "C" fn plugin_handle_wrapper(
            entry: $crate::NrStr,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            let entry_str = entry.as_str();
            match entry_str {
                $(
                    $entry_name => {
                        $handler_fn(sid, payload)
                    }
                )*
                _ => $crate::NrStatus::Invalid,
            }
        }

        unsafe extern "C" fn plugin_stream_data_wrapper(
            sid: u64,
            data: $crate::NrBytes,
        ) -> $crate::NrStatus {
            $(
                return $stream_data_fn(sid, data);
            )?
            #[allow(unreachable_code)]
            $crate::NrStatus::Unsupported
        }

        unsafe extern "C" fn plugin_stream_close_wrapper(
            sid: u64,
        ) -> $crate::NrStatus {
            $(
                return $stream_close_fn(sid);
            )?
            #[allow(unreachable_code)]
            $crate::NrStatus::Unsupported
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
    pub fn new(s: &str) -> Self {
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
        Self {
            key: NrStr::new(key),
            value,
        }
    }

    pub fn from_nr_str(key: NrStr, value: NrAny) -> Self {
        Self { key, value }
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
        if self.index.ptr.is_null() && self.entries.len >= 8 {
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
            let k = kv.key.as_str();
            self.index_insert(hash_str(k), i as u32);
        }
    }

    #[inline]
    fn should_grow(&self) -> bool {
        // Load factor approximately > 0.7 or too many tombstones
        if self.index.ptr.is_null() {
            return false;
        }
        let cap = self.index_len() as u32;
        (self.used + self.tomb) * 10 >= cap * 7
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
                2 => {
                    if first_tomb.is_none() {
                        first_tomb = Some(pos);
                    }
                }
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

        let kv = NrKVAny::new(key, value);
        self.entries.push(kv);

        self.ensure_index();
        if !self.index.ptr.is_null() {
            self.maybe_grow();
            let idx = (self.entries.len - 1) as u32;
            self.index_insert(hash_str(key), idx);
        }
    }

    pub fn insert_nr(&mut self, key: NrStr, value: NrAny) {
        let key_str = key.as_str();
        // If key exists, replace the value (set behavior)
        if let Some(v) = self.get_mut(key_str) {
            *v = value;
            return;
        }

        let kv = NrKVAny::from_nr_str(key, value);
        self.entries.push(kv);

        self.ensure_index();
        if !self.index.ptr.is_null() {
            self.maybe_grow();
            let idx = (self.entries.len - 1) as u32;
            self.index_insert(hash_str(key_str), idx);
        }
    }

    pub fn get(&self, key: &str) -> Option<&NrAny> {
        if self.index.ptr.is_null() {
            // Fallback to linear search (acceptable for small maps)
            for kv in self.entries.iter() {
                if kv.key.as_str() == key {
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
                    if kv.key.as_str() == key {
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
        if self.index.ptr.is_null() {
            for kv in self.entries.iter_mut() {
                if kv.key.as_str() == key {
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
                    if kv.key.as_str() == key {
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
        // Find the index of the entry to remove
        let idx = if self.index.ptr.is_null() {
            // Fallback to linear search
            self.entries.iter().position(|kv| kv.key.as_str() == key)?
        } else {
            // Use hash lookup
            let h = hash_str(key);
            let cap = self.index.len;
            let mask = cap - 1;
            let mut pos = (h as usize) & mask;
            let mut found_idx: Option<usize> = None;

            for _ in 0..cap {
                let slot = unsafe { &*self.index.ptr.add(pos) };
                match slot.state {
                    0 => break, // Empty slot found, key doesn't exist
                    1 if slot.hash == h => {
                        let entry_idx = slot.entry_idx as usize;
                        let kv = unsafe { &*self.entries.ptr.add(entry_idx) };
                        if kv.key.as_str() == key {
                            found_idx = Some(entry_idx);
                            break;
                        }
                    }
                    _ => {}
                }
                pos = (pos + 1) & mask;
            }

            found_idx?
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
            if !self.index.ptr.is_null() {
                let h_last = unsafe {
                    let kv = &*self.entries.ptr.add(idx);
                    hash_str(kv.key.as_str())
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
        if !self.index.ptr.is_null() {
            let h = hash_str(key);
            let cap = self.index.len;
            let mask = cap - 1;
            let mut pos = (h as usize) & mask;

            for _ in 0..cap {
                let slot = unsafe { &mut *self.index.ptr.add(pos) };
                match slot.state {
                    0 => break,
                    1 if slot.hash == h => {
                        let entry_idx = slot.entry_idx as usize;
                        if entry_idx == idx || (idx == last && entry_idx == last) {
                            slot.state = 2; // tombstone
                            self.used -= 1;
                            self.tomb += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                pos = (pos + 1) & mask;
            }

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
        if !self.index.ptr.is_null() {
            self.index.clear();
        }
        self.used = 0;
        self.tomb = 0;
    }
}

impl NrAny {
    pub fn new<T>(value: T, type_tag: u32) -> Self {
        let size = std::mem::size_of::<T>() as u64;
        let data = Box::into_raw(Box::new(value)) as *mut c_void;
        Self {
            data,
            size,
            type_tag,
            drop_fn: Some(drop_any::<T>),
        }
    }

    pub fn from_bytes(bytes: NrBytes, type_tag: u32) -> Self {
        let size = bytes.len;
        let data = if size > 0 {
            let v = bytes.as_slice().to_vec();
            Box::into_raw(Box::new(v)) as *mut c_void
        } else {
            std::ptr::null_mut()
        };
        Self {
            data,
            size,
            type_tag,
            drop_fn: Some(drop_bytes),
        }
    }

    pub fn as_ptr<T>(&self) -> Option<*const T> {
        if self.data.is_null() {
            None
        } else {
            Some(self.data as *const T)
        }
    }

    pub fn as_mut_ptr<T>(&mut self) -> Option<*mut T> {
        if self.data.is_null() {
            None
        } else {
            Some(self.data as *mut T)
        }
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

unsafe extern "C" fn drop_any<T>(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(ptr as *mut T);
        }
    }
}

unsafe extern "C" fn drop_bytes(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(ptr as *mut Vec<u8>);
        }
    }
}

impl Drop for NrAny {
    fn drop(&mut self) {
        if let Some(drop_fn) = self.drop_fn {
            if !self.data.is_null() {
                unsafe {
                    drop_fn(self.data);
                }
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
    pub fn from_nr_bytes(bytes: NrBytes) -> Self {
        let v = bytes.as_slice().to_vec();
        Self::from_vec(v)
    }
    pub fn from_string(s: String) -> Self {
        Self::from_vec(s.into_bytes())
    }
}

impl<T> NrVec<T> {
    pub fn from_vec(v: Vec<T>) -> Self {
        let mut v = std::mem::ManuallyDrop::new(v);
        let ptr = v.as_mut_ptr();
        let len = v.len();
        let cap = v.capacity();
        Self { ptr, len, cap }
    }

    pub fn into_vec(self) -> Vec<T> {
        let this = std::mem::ManuallyDrop::new(self);
        unsafe { Vec::from_raw_parts(this.ptr, this.len, this.cap) }
    }

    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.reserve(1);
        }
        unsafe {
            std::ptr::write(self.ptr.add(self.len), value);
        }
        self.len += 1;
    }

    pub fn clear(&mut self) {
        while self.len > 0 {
            self.len -= 1;
            unsafe {
                std::ptr::drop_in_place(self.ptr.add(self.len));
            }
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        let available = self.cap - self.len;
        if available < additional {
            let required = self.len + additional;
            let new_cap = if self.cap == 0 {
                std::cmp::max(1, required)
            } else {
                std::cmp::max(self.cap * 2, required)
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
}

impl<T> Drop for NrVec<T> {
    fn drop(&mut self) {
        if self.cap != 0 {
            if self.ptr.is_null() {
                return;
            }
            unsafe {
                // Drop elements
                let s = std::slice::from_raw_parts_mut(self.ptr, self.len);
                std::ptr::drop_in_place(s);

                // Deallocate
                if let Ok(layout) = std::alloc::Layout::array::<T>(self.cap) {
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
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.ptr.is_null() {
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
        let cap = this.cap;
        let len = this.len;

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

        let bytes = NrBytes::from_slice(b"test");
        let any_bytes = NrAny::from_bytes(bytes, 3);
        assert_eq!(any_bytes.type_tag(), 3);
        assert_eq!(any_bytes.size(), 4);

        let default_any = NrAny::default();
        assert!(default_any.is_null());
        assert_eq!(default_any.type_tag(), 0);
        assert_eq!(default_any.size(), 0);
    }
}
