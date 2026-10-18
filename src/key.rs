use std::fmt::Debug;

use bytes::Bytes;

pub const TIMESTAMP_ENABLED: bool = false;

/// Represents the key type used throughout the storage engine. Any type can be `Key` as
/// long as the type implement `AsRef`.
pub struct Key<T: AsRef<[u8]>>(T);

/// Represents the `[Key]` type as a slice with a lifetime `'a`.
pub type KeySlice<'a> = Key<&'a [u8]>;

/// Represents the `[Key]` type as a Vec<u8>.
pub type KeyVec = Key<Vec<u8>>;

/// Represents the `[Key]` type as Bytes.
pub type KeyBytes = Key<Bytes>;

impl<T: AsRef<[u8]>> Key<T> {
    /// Returns the inner type.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Returns the length of the underlying type.
    pub fn len(&self) -> usize {
        self.0.as_ref().len()
    }

    /// Returns whether the underlying type is empty.
    pub fn is_empty(&self) -> bool {
        self.0.as_ref().is_empty()
    }
}

impl Key<Vec<u8>> {
    pub fn from_vec(keys: Vec<u8>) -> Self {
        Self(keys)
    }

    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Clears all keys by removing them from the underlying vector.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Appends a slice to the end of the key.
    pub fn append(&mut self, data: &[u8]) {
        self.0.extend(data);
    }

    /// Set the key without reallocating.
    pub fn set_from_slice(&mut self, key_slice: KeySlice) {
        self.0.clear();
        self.0.extend_from_slice(key_slice.0);
    }

    /// Returns `Self` as a `KeySlice`.
    pub fn as_key_slice(&self) -> KeySlice<'_> {
        Key(self.0.as_slice())
    }

    /// Converts `Self` into `KeyBytes`.
    pub fn into_key_bytes(self) -> KeyBytes {
        Key(self.0.into())
    }

    /// Returns the underlying keys as a slice.
    pub fn raw_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Key<Bytes> {
    pub fn as_key_slice(&self) -> KeySlice<'_> {
        Key(&self.0)
    }

    /// Create a `KeyBytes` from a `Bytes`.
    pub fn from_bytes(bytes: Bytes) -> KeyBytes {
        Key(bytes)
    }

    pub fn raw_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl<'a> Key<&'a [u8]> {
    pub fn to_key_vec(self) -> KeyVec {
        Key(self.0.to_vec())
    }

    /// Create a key slice from a slice.
    pub fn from_slice(slice: &'a [u8]) -> Self {
        Self(slice)
    }

    pub fn raw_ref(self) -> &'a [u8] {
        self.0
    }
}

impl<T: AsRef<[u8]> + Debug> Debug for Key<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: AsRef<[u8]> + Default> Default for Key<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T: AsRef<[u8]> + PartialEq> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<T: AsRef<[u8]> + Eq> Eq for Key<T> {}

impl<T: AsRef<[u8]> + Clone> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: AsRef<[u8]> + Copy> Copy for Key<T> {}

impl<T: AsRef<[u8]> + PartialOrd> PartialOrd for Key<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<T: AsRef<[u8]> + Ord> Ord for Key<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}
