//! A runtime type registry for heterogeneous, dynamically typed data.
//!
//! The registry is keyed by [`TypeId`] and stores `Box<dyn Any>`, which is the
//! same trick `anymap` plays. It exists here so that `morphic` users do not
//! have to depend on a second crate for it, and so that lookups report a
//! consistent [`RegistryError`] instead of an opaque `None`.
//!
//! # Why not `inventory`
//!
//! `inventory` and `linkme` register values at link time, which is the right
//! tool for plugin discovery: the type set is closed at compile time, and
//! registration is free. A [`Registry`] is the complement, for the cases where
//! the set really is open at runtime — a document that names its own types, a
//! table loaded from a config file, a value carried across an FFI boundary.
//!
//! Unlike `anymap` (still `1.0.0-beta.2` after twelve years), this returns
//! typed errors and never silently replaces an entry.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Failure modes of [`Registry`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// No value of the requested type is registered.
    NotFound(TypeId),
    /// A value of that type is already registered, and the operation refused
    /// to replace it.
    AlreadyPresent(TypeId),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotFound(t) => write!(f, "no value registered for type {t:?}"),
            RegistryError::AlreadyPresent(t) => write!(f, "type {t:?} is already registered"),
        }
    }
}

impl std::error::Error for RegistryError {}

type Slot = Box<dyn Any + Send + Sync>;

/// A heterogeneous map from [`TypeId`] to concrete values.
#[derive(Default)]
pub struct Registry {
    slots: HashMap<TypeId, Slot>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `value` under its own type, replacing any previous entry.
    ///
    /// Returns the replaced value, or `None` if the type was absent.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.slots
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|old| old.downcast::<T>().ok().map(|b| *b))
    }

    /// Register `value`, failing with [`RegistryError::AlreadyPresent`] rather
    /// than replacing an existing entry.
    pub fn try_insert<T: Send + Sync + 'static>(&mut self, value: T) -> Result<(), RegistryError> {
        let id = TypeId::of::<T>();
        if self.slots.contains_key(&id) {
            return Err(RegistryError::AlreadyPresent(id));
        }
        self.slots.insert(id, Box::new(value));
        Ok(())
    }

    /// Borrow the registered value of type `T`.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.slots
            .get(&TypeId::of::<T>())
            .and_then(|s| s.downcast_ref::<T>())
    }

    /// Mutably borrow the registered value of type `T`.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.slots
            .get_mut(&TypeId::of::<T>())
            .and_then(|s| s.downcast_mut::<T>())
    }

    /// Borrow the registered value, or report [`RegistryError::NotFound`].
    pub fn try_get<T: Send + Sync + 'static>(&self) -> Result<&T, RegistryError> {
        self.get::<T>()
            .ok_or(RegistryError::NotFound(TypeId::of::<T>()))
    }

    /// Take the registered value out of the registry.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.slots
            .remove(&TypeId::of::<T>())
            .and_then(|s| s.downcast::<T>().ok().map(|b| *b))
    }

    /// Whether a value of type `T` is registered.
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.slots.contains_key(&TypeId::of::<T>())
    }

    /// Number of registered types.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Drop all entries.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// Iterate over the registered [`TypeId`]s, in unspecified order.
    pub fn type_ids(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.slots.keys().copied()
    }

    /// Iterate over registered values whose type is `R`, in unspecified order.
    ///
    /// `R` carries the same `Send + Sync + 'static` bounds as the inserting
    /// methods. Without them, `Slot` being `Box<dyn Any + Send + Sync>` would
    /// make `downcast_ref::<R>()` fail for every element and hand back a
    /// silently empty iterator — the absence a typed lookup is supposed to
    /// rule out.
    pub fn values<R: Send + Sync + 'static>(&self) -> impl Iterator<Item = &R> {
        self.slots.values().filter_map(|s| s.downcast_ref::<R>())
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("len", &self.slots.len())
            .finish()
    }
}
