use serde::Deserialize;
use std::error::Error;
use std::borrow::Borrow;

/// A string that is nonempty and has no null bytes
#[derive(Deserialize)]
#[serde(try_from = "String")]
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonEmptyNoNullString {
    inner: String
}
impl AsRef<str> for NonEmptyNoNullString {
    fn as_ref(&self) -> &str {
        self.inner.as_ref()
    }
}
impl Borrow<str> for NonEmptyNoNullString {
    fn borrow(&self) -> &str {
        self.inner.borrow()
    }
}
impl From<NonEmptyNoNullString> for String {
    fn from(nstr: NonEmptyNoNullString) -> Self {
        nstr.inner
    }
}
impl PartialEq<str> for NonEmptyNoNullString {
    fn eq(&self, other: &str) -> bool {
        self.inner == other
    }
}

/// The error returned when an empty or null-containing string is passed
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum TryIntoNonEmptyNoNullStringErr {
    /// The passed string is empty
    Empty,
    /// The passed string has a null byte at the given location
    HasNull(usize)
}
impl TryFrom<String> for NonEmptyNoNullString {
    type Error = TryIntoNonEmptyNoNullStringErr;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len()==0 {
            Err(TryIntoNonEmptyNoNullStringErr::Empty)
        } else if let Some(index) = value.as_bytes().iter().position(|c| *c==b'\x00') {
            Err(TryIntoNonEmptyNoNullStringErr::HasNull(index))
        } else {
            Ok(NonEmptyNoNullString {inner: value})
        }
    }
}
impl std::fmt::Display for TryIntoNonEmptyNoNullStringErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryIntoNonEmptyNoNullStringErr::Empty =>
                f.write_str("Empty string"),
            TryIntoNonEmptyNoNullStringErr::HasNull(i) =>
                write!(f, "String has null at {}", i),
        }
    }
}
impl Error for TryIntoNonEmptyNoNullStringErr {}
