//! The values the emulator tracks: integers, pointers into sized buffers, and
//! the buffers themselves (allocations). Everything the engine can't model is
//! `Unknown`, which keeps it honest rather than guessing.

/// Index of a buffer in the machine's heap.
pub type BufId = usize;

/// A value held by a variable or produced by an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    /// A concrete integer (modeled as `i128`; overflow is noted, not wrapped).
    Int(i128),
    /// A pointer `offset` bytes into buffer `buffer`.
    Ptr { buffer: BufId, offset: i128 },
    /// A null pointer.
    Null,
    /// Not modeled (an unmodeled call, a field, a cast the engine skips).
    Unknown,
}

impl Value {
    /// The integer this holds, if it is one.
    pub fn as_int(self) -> Option<i128> {
        match self {
            Value::Int(n) => Some(n),
            _ => None,
        }
    }

    /// Truthiness for a branch condition: non-zero int or non-null pointer is
    /// true, zero/null is false, `Unknown` is undecidable.
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Value::Int(n) => Some(n != 0),
            Value::Null => Some(false),
            Value::Ptr { .. } => Some(true),
            Value::Unknown => None,
        }
    }
}

/// An allocation: a flat region of `size` bytes (when known), with a label for
/// the UI (the variable and how it was allocated).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub id: BufId,
    /// Bytes, when the size could be computed.
    pub size: Option<i128>,
    /// e.g. `entry (malloc)` or `dst (parameter)`.
    pub label: String,
}
