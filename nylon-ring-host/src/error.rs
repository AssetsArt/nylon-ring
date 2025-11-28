use thiserror::Error;

/// Errors that can occur in the nylon-ring-host crate.
#[derive(Debug, Error)]
pub enum NylonRingHostError {
    #[error("failed to load plugin library: {0}")]
    FailedToLoadLibrary(#[source] libloading::Error),

    #[error("invalid plugin path: {0}")]
    InvalidPluginPath(String),

    #[error("missing required symbol: {0}")]
    MissingSymbol(String),

    #[error("plugin info pointer is null")]
    NullPluginInfo,

    #[error("incompatible ABI version: expected {expected}, got {actual}")]
    IncompatibleAbiVersion { expected: u32, actual: u32 },

    #[error("plugin vtable is null")]
    NullPluginVTable,

    #[error("plugin vtable missing required functions")]
    MissingRequiredFunctions,

    #[error("plugin init failed with status: {0:?}")]
    PluginInitFailed(nylon_ring::NrStatus),

    #[error("plugin handle failed immediately with status: {0:?}")]
    PluginHandleFailed(nylon_ring::NrStatus),

    #[error("failed to receive response from plugin: {0}")]
    ReceiveResponseFailed(String),

    #[error("mutex lock poisoned")]
    MutexPoisoned,

    #[error("oneshot channel closed")]
    OneshotClosed,
}
