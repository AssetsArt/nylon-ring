use thiserror::Error;

/// Errors that can occur in the nylon-ring-host crate.
#[derive(Debug, Error)]
pub enum NylonRingHostError {
    #[error("failed to load plugin library: {0}")]
    FailedToLoadLibrary(#[source] libloading::Error),

    #[error("missing required symbol: {0}")]
    MissingSymbol(String),

    #[error("plugin info pointer is null")]
    NullPluginInfo,

    #[error("incompatible ABI version: expected {expected}, got {actual}")]
    IncompatibleAbiVersion { expected: u32, actual: u32 },

    #[error("plugin info is too small: expected at least {expected} bytes, got {actual}")]
    IncompatiblePluginInfoSize { expected: u32, actual: u32 },

    #[error("plugin vtable is null")]
    NullPluginVTable,

    #[error("plugin vtable missing required functions")]
    MissingRequiredFunctions,

    #[error("plugin not found: {0}")]
    PluginNotFound(String),

    #[error("plugin init failed with status: {0:?}")]
    PluginInitFailed(nylon_ring::NrStatus),

    #[error("plugin handle failed immediately with status: {0:?}")]
    PluginHandleFailed(nylon_ring::NrStatus),

    #[error("plugin was unloaded before delivering its response")]
    PluginUnloaded,

    #[error("plugin panicked while handling the call")]
    PluginPanicked,

    #[error("synchronous plugin handler returned without delivering a response")]
    MissingSynchronousResponse,

    #[error("plugin call timed out")]
    Timeout,

    #[error("plugin calls did not drain before the grace period; {remaining} still active")]
    DrainTimeout { remaining: usize },

    #[error("plugin is busy with {active_calls} active calls")]
    PluginBusy { active_calls: usize },

    #[error("the synchronous fast path is already active on this thread")]
    FastPathReentrant,
}
