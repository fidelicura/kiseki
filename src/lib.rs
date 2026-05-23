#![doc = include_str!("../README.md")]

use std::io::Write as _;

//////////////////////////////////////////////////////////////////////
// Error
//////////////////////////////////////////////////////////////////////

/// Failure that can occur while writing a Kanata trace.
#[derive(Debug)]
pub enum Error {
    /// Underlying [`std::io::Write`] sink failed.
    Write(std::io::Error),

    /// A command referenced an [`Id`] that was never issued by
    /// [`Trace::start`] or has already been retired or flushed.
    UnknownId(Id),

    /// [`Trace::retire`] or [`Trace::flush`] was called for an [`Id`]
    /// that is not currently alive.
    DoubleRetire(Id),

    /// [`Trace::finish`] was called while one or more instructions
    /// had not yet been retired or flushed. The contained [`Vec`]
    /// lists every still-alive [`Id`].
    DanglingIds(Vec<Id>),

    /// A user-supplied string contained a tab, newline, or carriage
    /// return. Such characters cannot appear inside a Kanata field
    /// without breaking the line-based parser, so they are rejected
    /// up front when [`Config::SANITIZE`] is enabled. The contained
    /// [`String`] is the offending value.
    InvalidLabel(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write(error) => write!(f, "write failed: {error}"),
            Self::UnknownId(id) => write!(f, "unknown instruction id: {id}"),
            Self::DoubleRetire(id) => {
                write!(f, "instruction {id} retired or flushed more than once")
            }
            Self::DanglingIds(ids) => {
                write!(
                    f,
                    "trace finished with {} dangling instruction(s): {ids:?}",
                    ids.len()
                )
            }
            Self::InvalidLabel(label) => write!(f, "incompatible label name: {label:?}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Write(value)
    }
}

//////////////////////////////////////////////////////////////////////
// Result
//////////////////////////////////////////////////////////////////////

/// Convenience alias for results produced by this crate.
pub type Result<T = ()> = std::result::Result<T, Error>;

//////////////////////////////////////////////////////////////////////
// Config
//////////////////////////////////////////////////////////////////////

/// Compile-time configuration of a [`Trace`].
///
/// Implementing this trait lets a user pick the byte sink the trace
/// writes into, the type used to name pipeline stages, and whether
/// instruction lifecycle validation is performed.
pub trait Config {
    /// Byte sink the encoded trace is written into. Typically a
    /// [`Vec<u8>`], a [`std::io::BufWriter`] wrapping a file, or
    /// any other [`std::io::Write`] implementation.
    type Output: std::io::Write;

    /// Type used to name pipeline stages on `S` and `E` commands.
    /// Any [`std::fmt::Display`] type works; a `&'static str` or
    /// an enum implementing `Display` is the common choice.
    type Stage: std::fmt::Display;

    /// - `true`: [`Trace`] tracks the instructions lifecycle
    ///   and reports lifecycle violations as [`Error`]s.
    /// - `false`: the tracking and checks are compiled away.
    ///
    /// The crate's test suite only exercises the `true` (validating)
    /// path. The `false` path is a thin opt-out that bypasses the
    /// same code paths and is not covered.
    const VALIDATE: bool = true;

    /// - `true`: every label text passed to [`Trace::label`] is
    ///   scanned for characters that would corrupt the line-based
    ///   Kanata stream (`\t`, `\n`, `\r`) and rejected with
    ///   [`Error::InvalidLabel`] before any output is written.
    /// - `false`: the scan is compiled away and labels are emitted
    ///   verbatim. Use only when label text is already known to be
    ///   well-formed.
    const SANITIZE: bool = true;
}

//////////////////////////////////////////////////////////////////////
// Id
//////////////////////////////////////////////////////////////////////

/// Opaque handle representing an `INSN_ID_IN_FILE` value.
///
/// Issued by [`Trace::start`] and consumed by every later command
/// that targets the same instruction. The contained integer
/// matches the literal ID emitted into the Kanata log.
#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Id(u32);

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}", self.0))
    }
}

impl Id {
    /// Wrap a raw [`u32`] as an [`Id`]. Crate-internal only so users
    /// cannot fabricate a handle that was never issued by a trace.
    const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the wrapped [`u32`].
    const fn get(self) -> u32 {
        self.0
    }

    /// Return the current value, then increment `self` by one.
    /// Used to monotonically issue new ids.
    const fn bump(&mut self) -> Self {
        let old = self.0;
        self.0 = old + 1;
        Self(old)
    }

    /// Return the wrapped [`u32`] as an index ([`usize`]) for a bitmap.
    const fn index(self) -> usize {
        self.0 as _
    }
}

//////////////////////////////////////////////////////////////////////
// Lane
//////////////////////////////////////////////////////////////////////

/// Lane index used on `S` and `E` commands. Lane `0` represents the
/// main pipeline; higher lanes overlay extra information such as a
/// stall trail above the regular stages.
pub type Lane = u32;

//////////////////////////////////////////////////////////////////////
// Level
//////////////////////////////////////////////////////////////////////

/// Selects where a label is shown in the Konata viewer.
/// It is basically a `TYPE` field of an `L` command.
#[derive(Clone, Copy)]
pub enum Level {
    /// Shown in the left instruction pane (`L` type `0`). Typically
    /// the program counter together with the disassembled mnemonic.
    Pane,

    /// Shown as a mouse-over tooltip (`L` type `1`). Typically
    /// detailed information such as register values.
    Hover,
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pane => f.write_str("0"),
            Self::Hover => f.write_str("1"),
        }
    }
}

//////////////////////////////////////////////////////////////////////
// Validator
//////////////////////////////////////////////////////////////////////

/// Tracks which [`Id`]s are currently alive so that lifecycle bugs.
/// Bugs are surfaced as [`Error`]s instead of producing a bad log.
/// Active only when the parent [`Config::VALIDATE`] is `true`;
/// otherwise the call sites compile the check away.
struct Validator<C: Config> {
    /// Set of IDs that have been started.
    live: fixedbitset::FixedBitSet,

    /// Ties the validator's monomorphization to a specific [`Config`]
    /// so the `VALIDATE` constant can be consulted at compile time.
    phantom: std::marker::PhantomData<C>,
}

impl<C: Config> Default for Validator<C> {
    fn default() -> Self {
        Self {
            // live: fixedbitset::FixedBitSet::with_capacity(u16::MAX as usize),
            live: fixedbitset::FixedBitSet::new(),
            phantom: std::marker::PhantomData,
        }
    }
}

impl<C: Config> Validator<C> {
    /// Record a freshly issued `id` as alive.
    fn track(&mut self, id: Id) {
        let was_present = self.live.contains(id.index());
        self.live.grow_and_insert(id.index());
        debug_assert!(!was_present, "validator received a duplicate id: {id}");
    }

    /// Return `Ok` when `id` is currently alive,
    /// otherwise report it as [`Error::UnknownId`].
    fn check(&self, id: Id) -> Result {
        if id.index() < self.live.len() && self.live.contains(id.index()) {
            Ok(())
        } else {
            Err(Error::UnknownId(id))
        }
    }

    /// Mark `id` as no longer alive. This method returns
    /// [`Error::DoubleRetire`] when `id` was already absent.
    fn forget(&mut self, id: Id) -> Result {
        if !self.live.contains(id.index()) {
            return Err(Error::DoubleRetire(id));
        }
        self.live.remove(id.index());
        Ok(())
    }

    /// Return [`Error::DanglingIds`] when one or more
    /// [`Id`]s are still alive at the end of the trace.
    fn orphaned(&self) -> Result {
        let ids: Vec<Id> = self
            .live
            .ones()
            .map(|index| {
                #[allow(clippy::cast_possible_truncation)]
                let id = index as u32;
                Id::new(id)
            })
            .collect();
        ids.is_empty().then_some(()).ok_or(Error::DanglingIds(ids))
    }
}

//////////////////////////////////////////////////////////////////////
// Trace
//////////////////////////////////////////////////////////////////////

/// Field-value validator that rejects strings whose contents would
/// break the line-based Kanata format. Active only when the parent
/// [`Config::SANITIZE`] is `true`; otherwise the call sites compile
/// the check away.
struct Sanitizer;

impl Sanitizer {
    /// Render `text` to a [`String`] and reject it with
    /// [`Error::InvalidLabel`] when it contains any character that
    /// is forbidden inside a Kanata field (`\t`, `\n`, `\r`).
    fn sanitize<T>(text: &T) -> Result
    where
        T: std::fmt::Display,
    {
        let string = text.to_string();

        #[allow(clippy::items_after_statements)]
        const FORBIDDEN: &[&str] = &["\t", "\n", "\r"];
        for forbidden in FORBIDDEN {
            if string.contains(forbidden) {
                return Err(Error::InvalidLabel(string));
            }
        }

        Ok(())
    }
}

//////////////////////////////////////////////////////////////////////
// Trace
//////////////////////////////////////////////////////////////////////

/// Encoder that produces a Kanata-format processor pipeline trace.
///
/// A [`Trace`] is constructed with [`Trace::new`], driven by the
/// command methods and finalized by [`Trace::finish`], which
/// surrenders the underlying sink.
pub struct Trace<C: Config> {
    /// Sink the encoded trace is written into.
    output: C::Output,

    /// Lifecycle tracker. Inactive when `C::VALIDATE` is `false`.
    validator: Validator<C>,

    /// Next `INSN_ID_IN_FILE` value to hand out from [`Trace::start`].
    instruction_id: Id,

    /// Next `RETIRE_ID` value to use on an `R` command.
    retirement_id: Id,
}

impl<C: Config> Default for Trace<C>
where
    C::Output: Default,
{
    fn default() -> Self {
        let cycle = 0;
        let output = C::Output::default();
        Self::new(cycle, output).expect("default parameters should not panic")
    }
}

impl<C: Config> Trace<C> {
    fn write(&mut self, args: std::fmt::Arguments<'_>) -> Result {
        self.output.write_fmt(format_args!("{args}\n"))?;
        Ok(())
    }
}

impl<C: Config> Trace<C> {
    /// Create a new `Trace`, emit the Kanata header and a `C=`
    /// command anchoring the simulation at `cycle`.
    pub fn new(cycle: u32, output: C::Output) -> Result<Self> {
        let mut me = Self {
            output,
            validator: Validator::default(),
            instruction_id: Id::new(0),
            retirement_id: Id::new(0),
        };

        me.write(format_args!("Kanata\t0004"))?;
        me.write(format_args!("C=\t{cycle}"))?;

        Ok(me)
    }

    /// Conclude the trace and surrender the underlying sink so the
    /// produced bytes can be inspected, persisted, or further
    /// processed by the caller.
    ///
    /// When `C::VALIDATE` is enabled, this method first verifies that
    /// every instruction started by [`Trace::start`] has subsequently
    /// been retired or flushed. Outstanding instructions are reported
    /// as [`Error::DanglingIds`].
    #[must_use = "`finish` surrenders the underlying sink and runs final \
                  validation; dropping the result discards both the produced \
                  bytes and any `DanglingIds` diagnostic"]
    pub fn finish(self) -> Result<C::Output> {
        if C::VALIDATE {
            self.validator.orphaned()?;
        }

        Ok(self.output)
    }

    /// Emit a `C` command advancing the simulation clock by `delta`
    /// cycles relative to the previous command. Every subsequent
    /// command is interpreted as occurring in the resulting cycle
    /// until the next `advance` call.
    pub fn advance(&mut self, delta: u32) -> Result {
        self.write(format_args!("C\t{delta}"))
    }

    /// Emit an `I` command starting a new instruction. The simulator
    /// supplies `id` (its own `INSN_ID_IN_SIM`) and `thread`; this
    /// method allocates and returns the matching file-local [`Id`]
    /// that every subsequent command must use to refer back to the
    /// instruction.
    ///
    /// When `C::VALIDATE` is enabled, the freshly issued [`Id`] is
    /// recorded as alive so that later commands can be checked
    /// against it and so that [`Trace::finish`] can detect a missing
    /// retire or flush.
    #[must_use = "the returned `Id` is the only handle to the instruction \
                  just emitted; discarding it makes every later command for \
                  this instruction unreachable and guarantees `DanglingIds` \
                  on `finish`"]
    pub fn start(&mut self, id: u32, thread: u32) -> Result<Id> {
        let iid = self.instruction_id.bump();

        if C::VALIDATE {
            self.validator.track(iid);
        }

        self.write(format_args!("I\t{iid}\t{id}\t{thread}"))?;

        Ok(iid)
    }

    /// Emit an `E` command explicitly ending stage `stage` on
    /// `(id, lane)`. The Kanata specification allows omitting `E`
    /// when a stage is immediately followed by another, so it is
    /// only needed when a stage boundary must be pinpointed.
    ///
    /// When `C::VALIDATE` is enabled, `id` is checked against the
    /// live set and [`Error::UnknownId`] is returned if it is not
    /// currently alive.
    pub fn end(&mut self, id: Id, lane: Lane, stage: &C::Stage) -> Result {
        if C::VALIDATE {
            self.validator.check(id)?;
        }

        if C::SANITIZE {
            Sanitizer::sanitize(&stage)?;
        }

        self.write(format_args!("E\t{id}\t{lane}\t{stage}"))
    }

    /// Emit an `L` command attaching `text` to `id`. The `level`
    /// argument selects whether the text is displayed in the left
    /// instruction pane of the viewer or as a mouse-over tooltip.
    /// Repeated calls append to any text already attached.
    ///
    /// When `C::VALIDATE` is enabled, `id` is checked against the
    /// live set and [`Error::UnknownId`] is returned if it is not
    /// currently alive.
    pub fn label<T>(&mut self, id: Id, text: T, level: Level) -> Result
    where
        T: std::fmt::Display,
    {
        if C::VALIDATE {
            self.validator.check(id)?;
        }

        if C::SANITIZE {
            Sanitizer::sanitize(&text)?;
        }

        self.write(format_args!("L\t{id}\t{level}\t{text}"))
    }

    /// Emit an `S` command starting stage `stage` on `(id, lane)`.
    /// When `dependency` is `true`, an `_X` suffix is appended to
    /// the stage name so that any `W` arrows whose producer or
    /// consumer falls inside this stage are rendered by the viewer.
    ///
    /// When `C::VALIDATE` is enabled, `id` is checked against the
    /// live set and [`Error::UnknownId`] is returned if it is not
    /// currently alive.
    pub fn stage(&mut self, id: Id, lane: Lane, stage: &C::Stage, dependency: bool) -> Result {
        if C::VALIDATE {
            self.validator.check(id)?;
        }

        if C::SANITIZE {
            Sanitizer::sanitize(&stage)?;
        }

        let marker = if dependency { "_X" } else { <&str>::default() };
        self.write(format_args!("S\t{id}\t{lane}\t{stage}{marker}"))
    }

    /// Emit an `R` command of type `0`, retiring `id`. A fresh
    /// `RETIRE_ID` is allocated from the monotonic retirement
    /// counter, signalling that the instruction has committed its
    /// architectural effects.
    ///
    /// When `C::VALIDATE` is enabled, `id` is removed from the live
    /// set; [`Error::DoubleRetire`] is returned if it was already
    /// absent.
    pub fn retire(&mut self, id: Id) -> Result {
        if C::VALIDATE {
            self.validator.forget(id)?;
        }

        let rid = self.retirement_id.bump();
        self.write(format_args!("R\t{id}\t{rid}\t0"))
    }

    /// Emit an `R` command of type `1`, flushing `id`. The current
    /// `RETIRE_ID` counter is reused without bumping, matching the
    /// Kanata convention that a speculatively assigned retirement id
    /// may overlap with the next committed instruction.
    ///
    /// When `C::VALIDATE` is enabled, `id` is removed from the live
    /// set; [`Error::DoubleRetire`] is returned if it was already
    /// absent.
    pub fn flush(&mut self, id: Id) -> Result {
        if C::VALIDATE {
            self.validator.forget(id)?;
        }

        let rid = self.retirement_id.get();
        self.write(format_args!("R\t{id}\t{rid}\t1"))
    }

    /// Emit a `W` command recording a `producer → consumer` wake-up
    /// dependency. The viewer renders an arrow from the producer's
    /// dependency-marked stage to the consumer's; both endpoints
    /// must therefore have been emitted by [`Trace::stage`] with
    /// `dependency = true` for the arrow to be drawn.
    ///
    /// When `C::VALIDATE` is enabled, `consumer` is checked against
    /// the live set and [`Error::UnknownId`] is returned if it is no
    /// longer alive; the Kanata specification permits the producer
    /// to be either alive or already retired.
    pub fn wake(&mut self, consumer: Id, producer: Id) -> Result {
        if C::VALIDATE {
            self.validator.check(consumer)?;
        }

        self.write(format_args!("W\t{consumer}\t{producer}\t0"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Test [`Config`]. Both lifecycle validation and label
    /// sanitization run with their default `true` settings;
    /// `VALIDATE = false` and `SANITIZE = false` paths are
    /// intentionally out of scope and are not covered here.
    struct Cfg;

    impl Config for Cfg {
        type Output = Vec<u8>;
        type Stage = &'static str;
    }

    fn make(cycle: u32) -> Trace<Cfg> {
        let output = Vec::new();
        Trace::new(cycle, output).unwrap()
    }

    #[allow(clippy::unnecessary_wraps)]
    fn success(trace: Trace<Cfg>, expected: &'static str) -> Result {
        let bytes = trace.finish().unwrap();
        let actual = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(actual, expected);
        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    fn failure(trace: Trace<Cfg>, message: &'static str) -> Result {
        let error = trace.finish().unwrap_err().to_string();
        assert_eq!(&error, message);
        Ok(())
    }

    #[test]
    fn header_carries_initial_cycle() -> Result {
        let trace = make(42);
        success(trace, concat!("Kanata\t0004\n", "C=\t42\n",))
    }

    #[test]
    fn default_carries_cycle_zero() -> Result {
        let trace = Trace::<Cfg>::default();
        success(trace, concat!("Kanata\t0004\n", "C=\t0\n",))
    }

    #[test]
    fn advance_emits_delta() -> Result {
        let mut trace = make(0);
        trace.advance(3)?;
        trace.advance(1)?;
        success(
            trace,
            concat!("Kanata\t0004\n", "C=\t0\n", "C\t3\n", "C\t1\n",),
        )
    }

    #[test]
    fn start_hands_out_monotonic_ids() -> Result {
        let mut trace = make(0);
        let a = trace.start(10, 0)?;
        let b = trace.start(11, 1)?;
        let c = trace.start(12, 0)?;
        assert_eq!(a.get(), 0);
        assert_eq!(b.get(), 1);
        assert_eq!(c.get(), 2);
        trace.retire(a)?;
        trace.retire(b)?;
        trace.retire(c)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t10\t0\n",
                "I\t1\t11\t1\n",
                "I\t2\t12\t0\n",
                "R\t0\t0\t0\n",
                "R\t1\t1\t0\n",
                "R\t2\t2\t0\n",
            ),
        )
    }

    #[test]
    fn end_emits_stage_boundary() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"fetch", false)?;
        trace.end(id, 0, &"fetch")?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tfetch\n",
                "E\t0\t0\tfetch\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // `E` is only required when one stage is NOT immediately followed
    // by another. The Kanata format infers stage ends implicitly from
    // the next `S`, so emitting `E` is the exception, not the rule.
    #[test]
    fn stages_chain_without_explicit_end() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"fetch", false)?;
        trace.stage(id, 0, &"decode", false)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tfetch\n",
                "S\t0\t0\tdecode\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // `_X` suffix on stage name is a Kanata convention: only stages
    // marked with `X` are eligible producer/consumer endpoints for
    // `W` (wake) arrows. Without it, the viewer drops the arrow.
    #[test]
    fn stage_with_dependency_marker() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"exec", true)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\texec_X\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    #[test]
    fn lane_index_is_preserved() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 7, &"stall", false)?;
        trace.end(id, 7, &"stall")?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t7\tstall\n",
                "E\t0\t7\tstall\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // `L` type `0` lands in the left instruction pane, `L` type `1`
    // becomes a hover tooltip. The numeric encoding is the only thing
    // Kanata understands, so `Level` exists purely to give callers a
    // readable spelling.
    #[test]
    fn label_pane_and_hover_use_distinct_types() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.label(id, "0x100: addi", Level::Pane)?;
        trace.label(id, "rs1=5 rs2=7", Level::Hover)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "L\t0\t0\t0x100: addi\n",
                "L\t0\t1\trs1=5 rs2=7\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // Retire bumps the retirement counter; flush reuses the current
    // value without bumping. This mirrors the Kanata convention that
    // a speculative instruction's `RETIRE_ID` may overlap with the
    // next committed instruction, so the test exists to lock in that
    // quirk rather than test "real" behavior.
    #[test]
    fn flush_reuses_retirement_id_then_retire_bumps() -> Result {
        let mut trace = make(0);
        let a = trace.start(0, 0)?;
        let b = trace.start(1, 0)?;
        let c = trace.start(2, 0)?;
        trace.retire(a)?; // rid 0, bumps to 1
        trace.flush(b)?; // rid 1, no bump
        trace.retire(c)?; // rid 1, bumps to 2
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "I\t1\t1\t0\n",
                "I\t2\t2\t0\n",
                "R\t0\t0\t0\n",
                "R\t1\t1\t1\n",
                "R\t2\t1\t0\n",
            ),
        )
    }

    // By specification, a `W` arrow's producer is allowed to have
    // already retired by the time the consumer wakes up: only the
    // consumer must still be alive. This test pins down that
    // asymmetric validation rule.
    #[test]
    fn wake_allows_retired_producer() -> Result {
        let mut trace = make(0);
        let producer = trace.start(0, 0)?;
        let consumer = trace.start(1, 0)?;
        trace.retire(producer)?;
        trace.wake(consumer, producer)?;
        trace.retire(consumer)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "I\t1\t1\t0\n",
                "R\t0\t0\t0\n",
                "W\t1\t0\t0\n",
                "R\t1\t1\t0\n",
            ),
        )
    }

    #[test]
    fn stage_on_unknown_id_errors() {
        let mut trace = make(0);
        let bogus = Id::new(99);
        let err = trace.stage(bogus, 0, &"fetch", false).unwrap_err();
        assert!(matches!(err, Error::UnknownId(id) if id == bogus));
    }

    #[test]
    fn end_on_unknown_id_errors() {
        let mut trace = make(0);
        let err = trace.end(Id::new(5), 0, &"fetch").unwrap_err();
        assert!(matches!(err, Error::UnknownId(id) if id == Id::new(5)));
    }

    #[test]
    fn label_on_unknown_id_errors() {
        let mut trace = make(0);
        let err = trace.label(Id::new(5), "x", Level::Pane).unwrap_err();
        assert!(matches!(err, Error::UnknownId(id) if id == Id::new(5)));
    }

    #[test]
    fn wake_on_unknown_consumer_errors() {
        let mut trace = make(0);
        let err = trace.wake(Id::new(5), Id::new(0)).unwrap_err();
        assert!(matches!(err, Error::UnknownId(id) if id == Id::new(5)));
    }

    #[test]
    fn stage_after_retire_errors() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let err = trace.stage(id, 0, &"fetch", false).unwrap_err();
        assert!(matches!(err, Error::UnknownId(x) if x == id));
        Ok(())
    }

    #[test]
    fn double_retire_errors() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let err = trace.retire(id).unwrap_err();
        assert!(matches!(err, Error::DoubleRetire(x) if x == id));
        Ok(())
    }

    #[test]
    fn flush_after_retire_errors() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let err = trace.flush(id).unwrap_err();
        assert!(matches!(err, Error::DoubleRetire(x) if x == id));
        Ok(())
    }

    #[test]
    fn dangling_ids_reported_on_finish() -> Result {
        let mut trace = make(0);
        let _ = trace.start(0, 0)?;
        let _ = trace.start(1, 0)?;
        let bytes = trace.finish().unwrap_err();
        match bytes {
            Error::DanglingIds(mut ids) => {
                ids.sort();
                assert_eq!(ids, vec![Id::new(0), Id::new(1)]);
            }
            other => panic!("expected DanglingIds, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn unfinished_trace_message() -> Result {
        let mut trace = make(0);
        let _ = trace.start(0, 0)?;
        failure(
            trace,
            "trace finished with 1 dangling instruction(s): [Id(0)]",
        )
    }

    #[test]
    fn error_display_variants() {
        let io = std::io::Error::other("disk full");
        assert_eq!(format!("{}", Error::Write(io)), "write failed: disk full");
        assert_eq!(
            format!("{}", Error::UnknownId(Id::new(7))),
            "unknown instruction id: 7"
        );
        assert_eq!(
            format!("{}", Error::DoubleRetire(Id::new(3))),
            "instruction 3 retired or flushed more than once",
        );
    }

    #[test]
    fn io_error_converts_into_write() {
        let io = std::io::Error::other("nope");
        let err: Error = io.into();
        assert!(matches!(err, Error::Write(_)));
    }

    #[test]
    fn id_display_matches_inner() {
        assert_eq!(format!("{}", Id::new(0)), "0");
        assert_eq!(format!("{}", Id::new(12345)), "12345");
    }

    #[test]
    fn level_display_uses_numeric_encoding() {
        assert_eq!(format!("{}", Level::Pane), "0");
        assert_eq!(format!("{}", Level::Hover), "1");
    }

    #[test]
    fn advance_zero_emits_command() -> Result {
        let mut trace = make(0);
        trace.advance(0)?;
        success(trace, concat!("Kanata\t0004\n", "C=\t0\n", "C\t0\n",))
    }

    #[test]
    fn advance_large_delta() -> Result {
        let mut trace = make(0);
        trace.advance(u32::MAX)?;
        success(
            trace,
            concat!("Kanata\t0004\n", "C=\t0\n", "C\t4294967295\n",),
        )
    }

    #[test]
    fn initial_cycle_max_value() -> Result {
        let trace = make(u32::MAX);
        success(trace, concat!("Kanata\t0004\n", "C=\t4294967295\n",))
    }

    // Stage on lane 1 typically overlays stall/flush info above the
    // main pipeline on lane 0 for the same instruction.
    #[test]
    fn multi_lane_overlay_same_instruction() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"fetch", false)?;
        trace.stage(id, 1, &"stall", false)?;
        trace.end(id, 1, &"stall")?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tfetch\n",
                "S\t0\t1\tstall\n",
                "E\t0\t1\tstall\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // Retiring a sibling before starting a new instruction must not
    // reset the file-local id counter; ids are monotonic across the
    // entire trace lifetime.
    #[test]
    fn ids_keep_growing_after_retire() -> Result {
        let mut trace = make(0);
        let a = trace.start(0, 0)?;
        trace.retire(a)?;
        let b = trace.start(1, 0)?;
        trace.retire(b)?;
        assert_eq!(a.get(), 0);
        assert_eq!(b.get(), 1);
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "R\t0\t0\t0\n",
                "I\t1\t1\t0\n",
                "R\t1\t1\t0\n",
            ),
        )
    }

    #[test]
    fn label_between_stages_is_allowed() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"fetch", false)?;
        trace.label(id, "mid", Level::Hover)?;
        trace.stage(id, 0, &"decode", false)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tfetch\n",
                "L\t0\t1\tmid\n",
                "S\t0\t0\tdecode\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // One consumer waking from several producers needs one `W` per
    // edge; the consumer remains alive across all of them.
    #[test]
    fn multiple_wakes_for_one_consumer() -> Result {
        let mut trace = make(0);
        let p1 = trace.start(0, 0)?;
        let p2 = trace.start(1, 0)?;
        let consumer = trace.start(2, 0)?;
        trace.retire(p1)?;
        trace.wake(consumer, p1)?;
        trace.wake(consumer, p2)?;
        trace.retire(p2)?;
        trace.retire(consumer)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "I\t1\t1\t0\n",
                "I\t2\t2\t0\n",
                "R\t0\t0\t0\n",
                "W\t2\t0\t0\n",
                "W\t2\t1\t0\n",
                "R\t1\t1\t0\n",
                "R\t2\t2\t0\n",
            ),
        )
    }

    // The validator only tracks the consumer side of `W`, so a
    // producer id that was never issued is silently accepted. This
    // matches the Kanata spec, which permits referring to instructions
    // outside the local trace window.
    #[test]
    fn wake_accepts_unissued_producer() -> Result {
        let mut trace = make(0);
        let consumer = trace.start(0, 0)?;
        trace.wake(consumer, Id::new(999))?;
        trace.retire(consumer)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "W\t0\t999\t0\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // Flush is a valid terminal state: an instruction can leave the
    // pipeline without ever retiring. Validator must accept this as
    // a clean lifecycle close.
    #[test]
    fn flush_alone_closes_lifecycle() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.flush(id)?;
        success(
            trace,
            concat!("Kanata\t0004\n", "C=\t0\n", "I\t0\t0\t0\n", "R\t0\t0\t1\n",),
        )
    }

    #[test]
    fn advance_then_stage_uses_new_cycle_implicitly() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"fetch", false)?;
        trace.advance(1)?;
        trace.stage(id, 0, &"decode", false)?;
        trace.advance(2)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tfetch\n",
                "C\t1\n",
                "S\t0\t0\tdecode\n",
                "C\t2\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    // `INSN_ID_IN_SIM` and `thread` are pass-through fields. The
    // crate places no restriction on their values beyond `u32`.
    #[test]
    fn start_passes_through_sim_id_and_thread() -> Result {
        let mut trace = make(0);
        let id = trace.start(u32::MAX, 7)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t4294967295\t7\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    #[test]
    fn label_display_uses_user_impl() -> Result {
        struct Wrapped(u32);
        impl std::fmt::Display for Wrapped {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "wrapped({})", self.0)
            }
        }
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.label(id, Wrapped(42), Level::Pane)?;
        trace.retire(id)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "L\t0\t0\twrapped(42)\n",
                "R\t0\t0\t0\n",
            ),
        )
    }

    #[test]
    fn finish_after_success_returns_full_bytes() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let bytes = trace.finish()?;
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            "Kanata\t0004\nC=\t0\nI\t0\t0\t0\nR\t0\t0\t0\n",
        );
        Ok(())
    }

    // Pipeline-sized sanity check: three instructions through a small
    // 4-stage pipeline with a wake dependency from the first into the
    // third's execute stage.

    /// Test [`Config`] with lifecycle validation disabled.
    struct CfgNoValidate;

    impl Config for CfgNoValidate {
        type Output = Vec<u8>;
        type Stage = &'static str;
        const VALIDATE: bool = false;
        const SANITIZE: bool = true;
    }

    /// Test [`Config`] with label sanitization disabled.
    struct CfgNoSanitize;

    impl Config for CfgNoSanitize {
        type Output = Vec<u8>;
        type Stage = &'static str;
        const VALIDATE: bool = true;
        const SANITIZE: bool = false;
    }

    #[test]
    fn validate_disabled_bypasses_lifecycle_checks() -> Result {
        let mut trace = Trace::<CfgNoValidate>::new(0, Vec::new())?;
        let bogus = Id::new(99);
        trace.stage(bogus, 0, &"fetch", false)?;
        trace.end(bogus, 0, &"fetch")?;
        trace.label(bogus, "x", Level::Pane)?;
        trace.wake(bogus, Id::new(123))?;
        trace.retire(bogus)?;
        trace.flush(bogus)?;
        let bytes = trace.finish()?;
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("Kanata\t0004\nC=\t0\n"));
        assert!(text.contains("S\t99\t0\tfetch\n"));
        assert!(text.contains("R\t99\t0\t0\n"));
        Ok(())
    }

    #[test]
    fn validate_disabled_finish_ignores_dangling() -> Result {
        let mut trace = Trace::<CfgNoValidate>::new(0, Vec::new())?;
        let _ = trace.start(0, 0)?;
        let _ = trace.start(1, 0)?;
        let _ = trace.finish()?;
        Ok(())
    }

    #[test]
    fn sanitize_disabled_passes_forbidden_chars() -> Result {
        let mut trace = Trace::<CfgNoSanitize>::new(0, Vec::new())?;
        let id = trace.start(0, 0)?;
        trace.label(id, "tab\there", Level::Pane)?;
        trace.retire(id)?;
        let bytes = trace.finish()?;
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("L\t0\t0\ttab\there\n"));
        Ok(())
    }

    #[test]
    fn stage_sanitize_disabled_passes_forbidden_chars() -> Result {
        let mut trace = Trace::<CfgNoSanitize>::new(0, Vec::new())?;
        let id = trace.start(0, 0)?;
        trace.stage(id, 0, &"tab\there", false)?;
        trace.retire(id)?;
        let bytes = trace.finish()?;
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("S\t0\t0\ttab\there\n"));
        Ok(())
    }

    #[test]
    fn end_sanitize_disabled_passes_forbidden_chars() -> Result {
        let mut trace = Trace::<CfgNoSanitize>::new(0, Vec::new())?;
        let id = trace.start(0, 0)?;
        trace.end(id, 0, &"tab\there")?;
        trace.retire(id)?;
        let bytes = trace.finish()?;
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("E\t0\t0\ttab\there\n"));
        Ok(())
    }

    #[test]
    fn stage_validate_disabled_still_sanitizes() -> Result {
        let mut trace = Trace::<CfgNoValidate>::new(0, Vec::new())?;
        let err = trace
            .stage(Id::new(0), 0, &"bad\tstage", false)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "bad\tstage"));
        Ok(())
    }

    #[test]
    fn end_validate_disabled_still_sanitizes() -> Result {
        let mut trace = Trace::<CfgNoValidate>::new(0, Vec::new())?;
        let err = trace.end(Id::new(0), 0, &"bad\tstage").unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "bad\tstage"));
        Ok(())
    }

    #[test]
    fn stage_with_tab_is_rejected() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let err = trace.stage(id, 0, &"bad\tstage", false).unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "bad\tstage"));
        trace.retire(id)?;
        Ok(())
    }

    #[test]
    fn end_with_tab_is_rejected() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let err = trace.end(id, 0, &"bad\tstage").unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "bad\tstage"));
        trace.retire(id)?;
        Ok(())
    }

    #[test]
    fn label_with_tab_is_rejected() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let err = trace.label(id, "bad\ttext", Level::Pane).unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "bad\ttext"));
        trace.retire(id)?;
        Ok(())
    }

    #[test]
    fn label_with_newline_is_rejected() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let err = trace.label(id, "line\nbreak", Level::Pane).unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "line\nbreak"));
        trace.retire(id)?;
        Ok(())
    }

    #[test]
    fn label_with_carriage_return_is_rejected() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let err = trace.label(id, "cr\rhere", Level::Pane).unwrap_err();
        assert!(matches!(err, Error::InvalidLabel(s) if s == "cr\rhere"));
        trace.retire(id)?;
        Ok(())
    }

    #[test]
    fn rejected_label_does_not_emit() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        let _ = trace.label(id, "x\ty", Level::Pane);
        trace.retire(id)?;
        let bytes = trace.finish()?;
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains("x\ty"));
        assert!(!text.contains('L'));
        Ok(())
    }

    #[test]
    fn invalid_label_display() {
        let err = Error::InvalidLabel("bad\tlabel".into());
        assert_eq!(format!("{err}"), "incompatible label name: \"bad\\tlabel\"");
    }

    // Wake with a consumer that has already retired must
    // error since the consumer side requires liveness.
    #[test]
    fn wake_after_consumer_retired_errors() -> Result {
        let mut trace = make(0);
        let producer = trace.start(0, 0)?;
        let consumer = trace.start(1, 0)?;
        trace.retire(consumer)?;
        let err = trace.wake(consumer, producer).unwrap_err();
        assert!(matches!(err, Error::UnknownId(id) if id == consumer));
        trace.retire(producer)?;
        Ok(())
    }

    #[test]
    fn label_after_retire_errors() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let err = trace.label(id, "post", Level::Pane).unwrap_err();
        assert!(matches!(err, Error::UnknownId(x) if x == id));
        Ok(())
    }

    #[test]
    fn end_after_retire_errors() -> Result {
        let mut trace = make(0);
        let id = trace.start(0, 0)?;
        trace.retire(id)?;
        let err = trace.end(id, 0, &"fetch").unwrap_err();
        assert!(matches!(err, Error::UnknownId(x) if x == id));
        Ok(())
    }

    #[test]
    fn small_pipeline_three_instructions() -> Result {
        let mut trace = make(0);
        let a = trace.start(0, 0)?;
        trace.stage(a, 0, &"F", false)?;
        trace.advance(1)?;
        let b = trace.start(1, 0)?;
        trace.stage(a, 0, &"D", false)?;
        trace.stage(b, 0, &"F", false)?;
        trace.advance(1)?;
        let c = trace.start(2, 0)?;
        trace.stage(a, 0, &"X", true)?;
        trace.stage(b, 0, &"D", false)?;
        trace.stage(c, 0, &"F", false)?;
        trace.advance(1)?;
        trace.stage(c, 0, &"X", true)?;
        trace.wake(c, a)?;
        trace.retire(a)?;
        trace.retire(b)?;
        trace.retire(c)?;
        success(
            trace,
            concat!(
                "Kanata\t0004\n",
                "C=\t0\n",
                "I\t0\t0\t0\n",
                "S\t0\t0\tF\n",
                "C\t1\n",
                "I\t1\t1\t0\n",
                "S\t0\t0\tD\n",
                "S\t1\t0\tF\n",
                "C\t1\n",
                "I\t2\t2\t0\n",
                "S\t0\t0\tX_X\n",
                "S\t1\t0\tD\n",
                "S\t2\t0\tF\n",
                "C\t1\n",
                "S\t2\t0\tX_X\n",
                "W\t2\t0\t0\n",
                "R\t0\t0\t0\n",
                "R\t1\t1\t0\n",
                "R\t2\t2\t0\n",
            ),
        )
    }
}
