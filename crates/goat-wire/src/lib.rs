mod codec;
pub mod envelope;
pub mod peer;
pub mod transport;

pub use codec::{WireConn, WireError};
pub use envelope::{
    CallError, ErrorCode, Execution, Frame, Hello, Id, IdAllocator, Outcome, Role, StreamClass,
    envelope_fingerprint,
};
pub type EnvelopeConn<S> = WireConn<S, Frame, Frame>;
