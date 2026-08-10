//// A minimal, hand-rolled stand-in for `gleam/erlang/process.Subject`.
////
//// This package depends on neither `gleam_erlang` nor `gleam_otp` (the
//// remote-client feature this module exists for is built with no new Hex
//// dependencies at all — see `bsdkrun/client`), so the real `Subject` type
//// is not available. What is needed from it is small: a typed mailbox
//// address that `bsdkrun/client` can hand to a background WebSocket
//// connection process so it has somewhere to deliver subscription events,
//// and that the calling process can then block-read from with a timeout.
//// This module is exactly that and nothing more — it is not a general OTP
//// replacement, and every value it carries only ever flows between this
//// SDK's own processes.
////
//// A `Subject(a)` addresses one specific process's mailbox (the process
//// that called `new()`) and tags messages sent to it with a value unique to
//// that `Subject`, so `receive` can selectively pull out only the messages
//// meant for it — exactly like the real thing, and for the same reason:
//// BEAM's selective receive means unrelated messages already in the mailbox
//// are left untouched rather than consumed or misread.
////
//// The FFI surface (`bsdkrun_remote_ffi.erl`) is four one-line functions:
//// get the current pid, mint a unique tag, send a tagged message, and do a
//// timeout-bounded selective receive on a tag. Everything else is plain
//// Gleam.

/// An opaque handle to a BEAM process id. Only ever produced by `self()` and
/// consumed by the FFI functions below.
pub type Pid

@external(erlang, "bsdkrun_remote_ffi", "self")
pub fn self() -> Pid

@external(erlang, "bsdkrun_remote_ffi", "new_tag")
fn new_tag() -> String

/// A typed mailbox address, tagged so its messages can be picked out of the
/// owning process's mailbox without disturbing anything else sent to it.
pub opaque type Subject(a) {
  Subject(owner: Pid, tag: String)
}

/// A `Subject` addressing the *calling* process's own mailbox. Only the
/// process that created it should `receive` on it — like the real
/// `process.Subject`, sending to one from any process is fine, but reading
/// from one you did not create will simply time out, since the messages are
/// delivered to a different process's mailbox entirely.
pub fn new() -> Subject(a) {
  Subject(owner: self(), tag: new_tag())
}

/// The pid a `Subject` delivers to. Exposed so `bsdkrun/ws` can hand a
/// `Subject`'s address to the plain-Erlang WebSocket connection process,
/// which delivers events with `raw_send` directly rather than importing this
/// module back — the connection process is written in Erlang precisely
/// because it needs a blocking `receive` loop, which Gleam has no syntax for.
pub fn owner(subject: Subject(a)) -> Pid {
  subject.owner
}

/// The tag a `Subject` filters its mailbox on. See `owner`.
pub fn tag(subject: Subject(a)) -> String {
  subject.tag
}

@external(erlang, "bsdkrun_remote_ffi", "raw_send")
fn raw_send(owner: Pid, tag: String, message: a) -> Nil

/// Deliver `message` to `subject`'s owning process.
pub fn send(subject: Subject(a), message: a) -> Nil {
  raw_send(subject.owner, subject.tag, message)
}

@external(erlang, "bsdkrun_remote_ffi", "raw_receive")
fn raw_receive(tag: String, timeout_ms: Int) -> Result(a, Nil)

/// Block the calling process for up to `timeout_ms` waiting for a message
/// sent to `subject`. `Error(Nil)` means the timeout elapsed with nothing
/// delivered — this function never crashes on a missing message. A negative
/// `timeout_ms` waits forever (Erlang's `after infinity`).
///
/// Must be called from the same process that created `subject` (see `new`).
pub fn receive(subject: Subject(a), timeout_ms: Int) -> Result(a, Nil) {
  raw_receive(subject.tag, timeout_ms)
}
