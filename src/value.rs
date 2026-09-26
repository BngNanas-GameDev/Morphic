//! The dynamic [`Value`] type and its supporting object model.
//!
//! # Ownership model
//!
//! Every heap-allocated part of a [`Value`] lives in a garbage-collected
//! [`Gc`] cell, so arbitrarily cyclic graphs are expressible without leaking:
//! a list that contains itself is a perfectly ordinary `Value` here.
//!
//! Identity and equality are deliberately separate. [`Value::ptr_eq`] answers
//! "are these the same cell?", which is what a graph algorithm wants, while
//! [`PartialEq`] answers "do these look the same?", which is what a map key
//! wants.

use std::any::TypeId;
use std::fmt;
use std::rc::Rc;

use boa_gc::{Finalize, Gc, GcErased, GcRefCell, GcRefMut, Trace};

use crate::error::{Error, Result};

/// Maximum depth [`fmt::Display`] and [`fmt::Debug`] will descend into a
/// [`Value`] graph before printing `...`.
///
/// Cyclic graphs are legal, so any unbounded structural printer would hang.
pub const MAX_PRINT_DEPTH: usize = 64;

/// Maximum number of prototype links [`Object::get`], [`Object::has`],
/// [`Object::chain_len`] and [`Value::as_callable`] will follow before giving
/// up with [`Error::ProtoCycle`].
///
/// Prototype links are writable, so a cycle is constructible from safe code in
/// two calls. Without this bound, `a.obj_set_proto(&b); b.obj_set_proto(&a)`
/// followed by any property read spins forever at constant memory, which no
/// allocator-based watchdog can detect.
pub const MAX_PROTO_DEPTH: usize = 64;

/// Per-call state threaded through nested dynamic calls.
///
/// Carries the recursion depth so a cycle in the function graph produces an
/// [`Error::RecursionLimit`] instead of a native stack overflow.
///
/// # The depth counter is per-thread, not per-context
///
/// The counter lives in a thread-local, not in this struct, and that is
/// deliberate. An earlier design kept the depth in `CallCtx` and removed
/// `Value::call` so that every nested call had to share one context — but a
/// body can always construct a fresh `CallCtx::new()` and pass that down
/// instead, which silently resets the budget. That version overflowed the stack
/// in exactly the case the limit exists to prevent.
///
/// A thread-local cannot be reset by re-entering from inside a body, so the
/// limit holds no matter how the contexts are threaded. The trade-off is that
/// two `CallCtx`s on the same thread share one budget, and the smaller
/// `max_depth` wins at the point of entry.
#[derive(Debug, Clone)]
pub struct CallCtx {
    max_depth: u32,
}

thread_local! {
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl Default for CallCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl CallCtx {
    /// The default recursion limit, [`Self::DEFAULT_MAX_DEPTH`].
    pub const DEFAULT_MAX_DEPTH: u32 = 256;

    /// A fresh context with the default recursion limit.
    pub fn new() -> Self {
        Self::with_max_depth(Self::DEFAULT_MAX_DEPTH)
    }

    /// A fresh context that allows `max_depth` nested calls.
    pub fn with_max_depth(max_depth: u32) -> Self {
        Self { max_depth }
    }

    /// The configured recursion limit.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// The current nesting depth on this thread.
    pub fn depth(&self) -> u32 {
        DEPTH.with(std::cell::Cell::get)
    }

    /// Enter one call level, returning a guard that leaves it on drop.
    pub(crate) fn enter(&self) -> Result<DepthGuard> {
        DEPTH.with(|depth| {
            let current = depth.get();
            if current >= self.max_depth {
                return Err(Error::RecursionLimit {
                    limit: self.max_depth,
                });
            }
            depth.set(current + 1);
            Ok(DepthGuard)
        })
    }
}

/// Decrements the thread's call depth on drop, so an unwind through a panicking
/// body cannot leave the counter permanently raised and turn every later call
/// into a spurious [`Error::RecursionLimit`].
pub(crate) struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// The signature every [`NativeFn`] body must have.
pub type NativeFnBody = Rc<dyn Fn(&CallCtx, &[Value]) -> Result<Value>>;

/// A callable Rust closure that lives in the garbage-collected heap.
///
/// # A body may hold `Gc` cells, and that leaks
///
/// A body is an opaque `Rc<dyn Fn ..>`, so the collector cannot walk into it.
/// A [`Value::User`] payload is a bare [`GcErased`], and the two are equally
/// opaque. Neither can be introspected to discover which cells it holds.
///
/// The consequence is not the one you might expect. `boa_gc` 0.22 is not a
/// pure tracer: `GcHeader::is_rooted` is `non_root_count < ref_count`, and
/// `Trace::trace_non_roots` is what raises `non_root_count`. A `Gc` handle
/// living in ordinary Rust memory — a closure environment, a stack slot — is
/// therefore never marked non-root, so it is a **permanent root**. A cell
/// captured by a body cannot be freed while the body is alive.
///
/// That makes this sound, and it also makes it leak: when the `NativeFn` is
/// swept, its `Rc<dyn Fn ..>` is dropped under a guard, and `Gc::drop` only
/// runs `Finalize` when `finalizer_safe()`. The refcount is never decremented,
/// so the captured cell stays rooted forever.
///
/// In short: **a `Gc` captured by a body is safe but never reclaimed.** This is
/// a property of `boa_gc`'s refcount model, not of the [`Trace`] contract, and
/// it may change in a future `boa_gc`. If you need captured cells actually
/// collected, use [`gc-arena`](https://crates.io/crates/gc-arena), whose
/// mutation-XOR-collection design has no rooting problem at all.
///
/// # Examples
///
/// ```
/// use morphic::{CallCtx, Gc, NativeFn, Value};
///
/// let target = Gc::new(vec![1i64, 2, 3]);
/// let doubled = NativeFn::from_user("doubled", target, |v, _ctx, _args| {
///     Ok(Value::list_from(v.iter().map(|x| Value::int(x * 2))))
/// });
///
/// let mut ctx = CallCtx::new();
/// let out = Value::native(doubled).call_with(&mut ctx, &[]).unwrap();
/// assert_eq!(out.list_len().unwrap(), 3);
/// assert_eq!(out.list_get(2).unwrap().as_int(), Some(6));
/// ```
pub struct NativeFn {
    name: Gc<String>,
    body: NativeFnBody,
}

impl Finalize for NativeFn {}

unsafe impl Trace for NativeFn {
    // SAFETY: only `name` can hold a `Gc`, and it is traced. The opaque `body`
    // is deliberately not traced. That is sound because `boa_gc` 0.22 roots by
    // reference count, so a `Gc` in the body stays rooted regardless; see the
    // type-level docs for why that is also why captured cells leak.
    boa_gc::custom_trace!(this, mark, {
        mark(&this.name);
    });
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeFn")
            .field("name", &self.name)
            .finish()
    }
}

impl NativeFn {
    /// Wrap `body` as a named native function.
    ///
    /// `body` may capture `Gc` cells. Doing so is sound but leaks them once this
    /// `NativeFn` is collected; see the type-level docs for the mechanism.
    pub fn new<F>(name: &str, body: F) -> Self
    where
        F: Fn(&CallCtx, &[Value]) -> Result<Value> + 'static,
    {
        Self {
            name: Gc::new(name.to_owned()),
            body: Rc::new(body),
        }
    }

    /// Build a native function that receives `target` as its `self`.
    ///
    /// This is the ergonomic way to give a [`Value::User`] payload behaviour:
    /// the data cell is moved into the body in one step, so it cannot be
    /// forgotten.
    ///
    /// The body gets shared access. Use [`NativeFn::from_user_mut`] when it
    /// needs to mutate.
    pub fn from_user<T, F>(name: &str, target: Gc<T>, body: F) -> Self
    where
        T: Trace + 'static,
        F: Fn(&T, &CallCtx, &[Value]) -> Result<Value> + 'static,
    {
        Self::new(name, move |ctx, args| body(&target, ctx, args))
    }

    /// Build a native function that receives a mutable borrow of `target`.
    ///
    /// The borrow is taken for the duration of the call only, and a conflicting
    /// borrow is reported as [`Error::BorrowConflict`] rather than panicking.
    pub fn from_user_mut<T, F>(name: &str, target: Gc<GcRefCell<T>>, body: F) -> Self
    where
        T: Trace + 'static,
        F: Fn(GcRefMut<'_, T>, &CallCtx, &[Value]) -> Result<Value> + 'static,
    {
        Self::new(name, move |ctx, args| {
            let borrowed = target
                .try_borrow_mut()
                .map_err(|_| Error::BorrowConflict("a captured user cell"))?;
            body(borrowed, ctx, args)
        })
    }

    /// The function's name.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Invoke the body.
    ///
    /// The caller is responsible for depth accounting; [`Value::call_with`]
    /// does that for you.
    pub fn call(&self, ctx: &CallCtx, args: &[Value]) -> Result<Value> {
        (self.body)(ctx, args)
    }
}

/// An insertion-ordered association list from [`Value`] to [`Value`].
///
/// A `Vec`-backed map rather than a `HashMap` for two reasons: keys keep a
/// stable iteration order, and [`Value`] deliberately does not implement
/// `Hash`, because a total, reflexive equality is what map keys need and
/// hashing would force a choice this crate does not want to make.
///
/// Lookup is linear. For a config or plugin table that is the right trade; it
/// is not a database.
#[derive(Clone, Default, Trace, Finalize)]
pub struct ValueMap {
    entries: Vec<(Value, Value)>,
}

impl ValueMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up `key`.
    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Look up a string key.
    pub fn get_str(&self, key: &str) -> Option<&Value> {
        self.get(&Value::str(key))
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &Value) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// Insert `value` under `key`, returning the previous value if any.
    ///
    /// An existing key keeps its original position, so iteration order
    /// reflects first-insertion, not last-write.
    pub fn insert(&mut self, key: Value, value: Value) -> Option<Value> {
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            return Some(core::mem::replace(&mut slot.1, value));
        }
        self.entries.push((key, value));
        None
    }

    /// Insert, failing with [`Error::DuplicateKey`] if `key` is already present.
    pub fn try_insert(&mut self, key: Value, value: Value) -> Result<()> {
        if self.contains_key(&key) {
            return Err(Error::DuplicateKey);
        }
        self.entries.push((key, value));
        Ok(())
    }

    /// Remove `key`, returning its value.
    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    /// Remove a string key, returning its value.
    pub fn remove_str(&mut self, key: &str) -> Option<Value> {
        self.remove(&Value::str(key))
    }

    /// Iterate over entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    /// Iterate over keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(k, _)| k)
    }

    /// Iterate over values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }

    /// Drop all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl fmt::Debug for ValueMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// A property bag with an optional prototype chain.
///
/// Models the record/object shape that dynamic code expects: named properties,
/// a class name for error messages and formatting, and inheritance by
/// delegation to a parent object.
#[derive(Clone, Default, Trace, Finalize)]
pub struct Object {
    class: Option<Gc<String>>,
    props: ValueMap,
    proto: Option<Gc<GcRefCell<Object>>>,
}

impl Object {
    /// An object with no class, no prototype and no properties.
    pub fn new() -> Self {
        Self::default()
    }

    /// An object carrying a class name.
    pub fn with_class(class: &str) -> Self {
        let mut obj = Self::new();
        obj.set_class(class);
        obj
    }

    /// The class name, if set.
    pub fn class(&self) -> Option<&str> {
        self.class.as_ref().map(|s| s.as_str())
    }

    /// Set the class name.
    pub fn set_class(&mut self, class: &str) {
        self.class = Some(Gc::new(class.to_owned()));
    }

    /// The prototype object, if set.
    pub fn proto(&self) -> Option<Gc<GcRefCell<Object>>> {
        self.proto.clone()
    }

    /// Set the prototype object.
    pub fn set_proto(&mut self, proto: Gc<GcRefCell<Object>>) {
        self.proto = Some(proto);
    }

    /// Look up a property on this object only.
    pub fn get_local(&self, key: &str) -> Option<&Value> {
        self.props.get_str(key)
    }

    /// Look up a property, following the prototype chain.
    ///
    /// Returns a clone rather than a reference: the property may live on a
    /// prototype in a different cell, and holding a borrow of that cell while
    /// handing out a `&Value` into it would be unsound.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BorrowConflict`] if any object on the chain is
    /// currently mutably borrowed, and [`Error::ProtoCycle`] if the chain is
    /// longer than [`MAX_PROTO_DEPTH`] or loops. The cycle bound is essential:
    /// prototype links are writable, so `a.obj_set_proto(&b)` followed by
    /// `b.obj_set_proto(&a)` is legal and would otherwise loop forever.
    pub fn get(&self, key: &str) -> Result<Option<Value>> {
        if let Some(v) = self.props.get_str(key) {
            return Ok(Some(v.clone()));
        }
        self.walk(key).map(Some)
    }

    /// Walk the prototype chain looking for `key`.
    fn walk(&self, key: &str) -> Result<Value> {
        let mut cur = self.proto.clone();
        let mut steps = 0usize;
        while let Some(cell) = cur {
            steps += 1;
            if steps > MAX_PROTO_DEPTH {
                return Err(Error::ProtoCycle {
                    limit: MAX_PROTO_DEPTH,
                });
            }
            let borrowed = cell
                .try_borrow()
                .map_err(|_| Error::BorrowConflict("a prototype"))?;
            if let Some(v) = borrowed.props.get_str(key) {
                return Ok(v.clone());
            }
            cur = borrowed.proto();
        }
        Err(Error::KeyNotFound)
    }

    /// Look up a property anywhere on the prototype chain.
    pub fn lookup(&self, key: &str) -> Result<Option<Value>> {
        self.get(key)
    }

    /// Set a property on this object, shadowing any prototype's.
    pub fn set(&mut self, key: &str, value: Value) -> Option<Value> {
        self.props.insert(Value::str(key), value)
    }

    /// Remove a property from this object.
    ///
    /// A property inherited from the prototype becomes visible again.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.props.remove_str(key)
    }

    /// Whether a property is reachable, including through the prototype chain.
    ///
    /// # Errors
    ///
    /// Propagates [`Error::BorrowConflict`] and [`Error::ProtoCycle`] from
    /// [`Object::get`], so a temporarily unreadable prototype is never reported
    /// as "absent".
    pub fn has(&self, key: &str) -> Result<bool> {
        self.get(key).map(|v| v.is_some())
    }
    /// Number of own properties, excluding the prototype chain.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    /// Whether this object has no own properties.
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Iterate over own properties in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.props.iter()
    }

    /// Own property names, in insertion order.
    pub fn keys(&self) -> Vec<&str> {
        self.props
            .keys()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
    }

    /// Drop all own properties.
    pub fn clear(&mut self) {
        self.props.clear();
    }

    /// Number of objects in the prototype chain, including this one.
    ///
    /// # Errors
    ///
    /// [`Error::ProtoCycle`] if the chain loops, and [`Error::BorrowConflict`]
    /// if a link cannot be read. It never reports a short count.
    pub fn chain_len(&self) -> Result<usize> {
        let mut n = 1;
        let mut cur = self.proto.clone();
        while let Some(cell) = cur {
            if n > MAX_PROTO_DEPTH {
                return Err(Error::ProtoCycle {
                    limit: MAX_PROTO_DEPTH,
                });
            }
            n += 1;
            cur = cell
                .try_borrow()
                .map_err(|_| Error::BorrowConflict("a prototype"))?
                .proto();
        }
        Ok(n)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Object")
            .field("class", &self.class())
            .field("props", &self.props)
            .finish()
    }
}

/// The property name an object must expose to be callable via
/// [`Value::call_with`].
pub const CALL_PROP: &str = "__call__";

/// A dynamically typed value whose heap parts are garbage collected.
///
/// # Shape
///
/// `Value` is a closed enum, so every match on it stays exhaustive and no
/// `dyn` is needed for *data*. Extensibility comes from two directions:
/// [`Value::User`] for opaque Rust payloads that participate in the heap
/// graph, and [`Value::Native`] for behaviour.
///
/// # Equality
///
/// [`PartialEq`] is a *total* equality so that `Value` can key a
/// [`ValueMap`]. Two consequences worth knowing:
///
/// * Floats compare by bit pattern, so `NaN == NaN` holds and reflexivity is
///   preserved. `Int`/`Float` pairs compare numerically, so `1` and `1.0` are
///   the same key.
/// * `List`, `Map`, `Object`, `Native` and `User` compare by *identity*, not
///   structure. Structural equality on a cyclic graph is not decidable, so
///   structural comparison is deliberately not offered.
///
/// Mixed `Int`/`Float` transitivity has the usual float wart:
/// `Int(2^53 + 1) == Float(2^53)` while `Int(2^53 + 1) != Int(2^53)`. Do not
/// rely on `Eq`-style transitivity across numeric kinds.
#[derive(Clone, Trace, Finalize)]
#[non_exhaustive]
pub enum Value {
    /// Absence of a value.
    Nil,
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A Unicode scalar value.
    Char(char),
    /// An immutable string.
    Str(Gc<String>),
    /// An immutable byte string.
    Bytes(Gc<Vec<u8>>),
    /// A growable, possibly cyclic sequence.
    List(Gc<GcRefCell<Vec<Value>>>),
    /// An insertion-ordered key/value table.
    Map(Gc<GcRefCell<ValueMap>>),
    /// A property bag with a prototype chain.
    Object(Gc<GcRefCell<Object>>),
    /// A callable Rust closure.
    Native(Gc<NativeFn>),
    /// An opaque Rust payload living in the collected heap.
    ///
    /// Inert: it carries data, not behaviour. Recover it with
    /// [`Value::as_user`], and give it behaviour by wrapping it in a
    /// [`NativeFn::from_user`].
    User(GcErased),
    /// A [`TypeId`], for type-driven dispatch.
    Type(TypeId),
}

impl Value {
    /// The empty value.
    pub const NIL: Self = Value::Nil;

    /// Build a boolean.
    pub fn bool(v: bool) -> Self {
        Value::Bool(v)
    }

    /// Build an integer.
    pub fn int(v: i64) -> Self {
        Value::Int(v)
    }

    /// Build a float.
    pub fn float(v: f64) -> Self {
        Value::Float(v)
    }

    /// Build a character.
    pub fn char(v: char) -> Self {
        Value::Char(v)
    }

    /// Build a string.
    pub fn str(v: &str) -> Self {
        Value::Str(Gc::new(v.to_owned()))
    }

    /// Build a string by cloning an owned `String`.
    pub fn string(v: String) -> Self {
        Value::Str(Gc::new(v))
    }

    /// Build a byte string.
    pub fn bytes(v: impl Into<Vec<u8>>) -> Self {
        Value::Bytes(Gc::new(v.into()))
    }

    /// Build an empty list.
    pub fn list() -> Self {
        Value::List(Gc::new(GcRefCell::new(Vec::new())))
    }

    /// Build a list from an iterator.
    pub fn list_from<I: IntoIterator<Item = Value>>(items: I) -> Self {
        Value::List(Gc::new(GcRefCell::new(items.into_iter().collect())))
    }

    /// Build an empty map.
    pub fn map() -> Self {
        Value::Map(Gc::new(GcRefCell::new(ValueMap::new())))
    }

    /// Build a map from key/value pairs.
    pub fn map_from<K: Into<Value>, V: Into<Value>>(
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut m = ValueMap::new();
        for (k, v) in entries {
            m.insert(k.into(), v.into());
        }
        Value::Map(Gc::new(GcRefCell::new(m)))
    }

    /// Build an empty object.
    pub fn object() -> Self {
        Value::Object(Gc::new(GcRefCell::new(Object::new())))
    }

    /// Build an object with a class name.
    pub fn object_of_class(class: &str) -> Self {
        Value::Object(Gc::new(GcRefCell::new(Object::with_class(class))))
    }

    /// Build a native function value.
    pub fn native(f: NativeFn) -> Self {
        Value::Native(Gc::new(f))
    }

    /// Move any traced Rust payload into the collected heap.
    ///
    /// `T` must implement both [`Trace`] and [`Finalize`], and
    /// `#[derive(Trace)]` alone does not imply `Finalize`:
    ///
    /// ```
    /// use morphic::{Finalize, Trace, Value};
    ///
    /// #[derive(Trace, Finalize)]
    /// struct Point {
    ///     x: f64,
    ///     y: f64,
    /// }
    ///
    /// let v = Value::user(Point { x: 1.0, y: 2.0 });
    /// assert_eq!(v.as_user::<Point>().unwrap().x, 1.0);
    /// ```
    pub fn user<T: Trace + 'static>(v: T) -> Self {
        Value::User(GcErased::new(Gc::new(v)))
    }

    /// Build a `TypeId` value.
    pub fn type_id(id: TypeId) -> Self {
        Value::Type(id)
    }

    /// A short name for this value's shape, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Char(_) => "char",
            Value::Str(_) => "str",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Object(_) => "object",
            Value::Native(_) => "native",
            Value::User(_) => "user",
            Value::Type(_) => "type",
        }
    }

    /// Whether this is [`Value::Nil`].
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Whether this is a [`Value::Native`], or an object exposing
    /// [`CALL_PROP`].
    pub fn is_callable(&self) -> bool {
        match self {
            Value::Native(_) => true,
            Value::Object(o) => o
                .try_borrow()
                .ok()
                .and_then(|b| b.get(CALL_PROP).ok().flatten())
                .is_some_and(|v| v.is_callable()),
            _ => false,
        }
    }
    /// Whether two values are the same heap cell.
    ///
    /// Meaningful for `List`, `Map`, `Object`, `Native` and `User`, where
    /// [`PartialEq`] is already identity. For the scalar variants this is
    /// `PartialEq`.
    pub fn ptr_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::List(a), Value::List(b)) => Gc::ptr_eq(a, b),
            (Value::Map(a), Value::Map(b)) => Gc::ptr_eq(a, b),
            (Value::Object(a), Value::Object(b)) => Gc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => Gc::ptr_eq(a, b),
            (Value::User(a), Value::User(b)) => GcErased::ptr_eq(a, b),
            _ => self == other,
        }
    }

    /// The [`TypeId`] of a [`Value::User`] payload.
    pub fn user_type_id(&self) -> Option<TypeId> {
        match self {
            Value::User(u) => Some(u.type_id()),
            _ => None,
        }
    }

    /// The [`TypeId`] carried by a [`Value::Type`].
    pub fn as_type_id(&self) -> Option<TypeId> {
        match self {
            Value::Type(t) => Some(*t),
            _ => None,
        }
    }

    fn mismatch<T>(&self, context: &'static str, expected: &'static str) -> Result<T> {
        Err(Error::TypeMismatch {
            context,
            expected,
            found: self.type_name(),
        })
    }

    /// Borrow a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Borrow an integer.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Borrow a float, widening an [`Value::Int`] if needed.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Borrow a character.
    pub fn as_char(&self) -> Option<char> {
        match self {
            Value::Char(c) => Some(*c),
            _ => None,
        }
    }

    /// Borrow a string slice.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Borrow a byte slice.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    /// Borrow the backing list cell.
    pub fn as_list(&self) -> Option<&GcRefCell<Vec<Value>>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    /// Borrow the backing map cell.
    pub fn as_map(&self) -> Option<&GcRefCell<ValueMap>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Borrow the backing object cell.
    pub fn as_object(&self) -> Option<&GcRefCell<Object>> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Borrow the native function.
    pub fn as_native(&self) -> Option<&NativeFn> {
        match self {
            Value::Native(f) => Some(f),
            _ => None,
        }
    }

    /// Borrow the native function behind an object, via the prototype chain.
    ///
    /// # Errors
    ///
    /// Propagates [`Error::BorrowConflict`] and [`Error::ProtoCycle`] rather
    /// than reporting a callable object as non-callable.
    pub fn as_callable(&self) -> Result<Gc<NativeFn>> {
        match self {
            Value::Native(f) => Ok(f.clone()),
            Value::Object(o) => {
                let borrowed = o
                    .try_borrow()
                    .map_err(|_| Error::BorrowConflict("an object"))?;
                let found = match borrowed.get(CALL_PROP) {
                    Ok(found) => found,
                    // No `__call__` anywhere on the chain means "not callable",
                    // not "the key is missing". `BorrowConflict` and
                    // `ProtoCycle` still propagate.
                    Err(Error::KeyNotFound) => return Err(Error::NotCallable),
                    Err(other) => return Err(other),
                };
                // `Value` has a `Drop` impl (the derive adds one), so a partial
                // move out of it is not allowed. Borrow instead of destructuring.
                let native = match &found {
                    Some(Value::Native(f)) => Some(f.clone()),
                    _ => None,
                };
                native.ok_or(Error::NotCallable)
            }
            _ => Err(Error::NotCallable),
        }
    }

    /// Recover a [`Value::User`] payload as its concrete type.
    pub fn as_user<T: Trace + 'static>(&self) -> Option<Gc<T>> {
        match self {
            Value::User(u) => u.clone().downcast::<T>(),
            _ => None,
        }
    }

    /// Whether the [`Value::User`] payload is a `T`.
    pub fn is_user<T: Trace + 'static>(&self) -> bool {
        match self {
            Value::User(u) => u.is::<T>(),
            _ => false,
        }
    }

    /// Run `f` with mutable access to a list's backing storage.
    pub fn with_list<R>(&self, f: impl FnOnce(&mut Vec<Value>) -> R) -> Result<R> {
        let cell = match self {
            Value::List(l) => l,
            other => return other.mismatch("Value::with_list", "list"),
        };
        let mut guard = cell
            .try_borrow_mut()
            .map_err(|_| Error::BorrowConflict("a list"))?;
        Ok(f(&mut guard))
    }

    /// Run `f` with read access to a list's backing storage.
    pub fn with_list_ref<R>(&self, f: impl FnOnce(&[Value]) -> R) -> Result<R> {
        let cell = match self {
            Value::List(l) => l,
            other => return other.mismatch("Value::with_list_ref", "list"),
        };
        let guard = cell
            .try_borrow()
            .map_err(|_| Error::BorrowConflict("a list"))?;
        Ok(f(&guard))
    }

    /// Run `f` with mutable access to a map's backing storage.
    pub fn with_map<R>(&self, f: impl FnOnce(&mut ValueMap) -> R) -> Result<R> {
        let cell = match self {
            Value::Map(m) => m,
            other => return other.mismatch("Value::with_map", "map"),
        };
        let mut guard = cell
            .try_borrow_mut()
            .map_err(|_| Error::BorrowConflict("a map"))?;
        Ok(f(&mut guard))
    }

    /// Run `f` with read access to a map's backing storage.
    pub fn with_map_ref<R>(&self, f: impl FnOnce(&ValueMap) -> R) -> Result<R> {
        let cell = match self {
            Value::Map(m) => m,
            other => return other.mismatch("Value::with_map_ref", "map"),
        };
        let guard = cell
            .try_borrow()
            .map_err(|_| Error::BorrowConflict("a map"))?;
        Ok(f(&guard))
    }

    /// Run `f` with mutable access to an object's backing storage.
    pub fn with_object<R>(&self, f: impl FnOnce(&mut Object) -> R) -> Result<R> {
        let cell = match self {
            Value::Object(o) => o,
            other => return other.mismatch("Value::with_object", "object"),
        };
        let mut guard = cell
            .try_borrow_mut()
            .map_err(|_| Error::BorrowConflict("an object"))?;
        Ok(f(&mut guard))
    }

    /// Run `f` with read access to an object's backing storage.
    pub fn with_object_ref<R>(&self, f: impl FnOnce(&Object) -> R) -> Result<R> {
        let cell = match self {
            Value::Object(o) => o,
            other => return other.mismatch("Value::with_object_ref", "object"),
        };
        let guard = cell
            .try_borrow()
            .map_err(|_| Error::BorrowConflict("an object"))?;
        Ok(f(&guard))
    }

    /// Length of a list.
    pub fn list_len(&self) -> Result<usize> {
        self.with_list_ref(|l| l.len())
    }

    /// Append to a list.
    pub fn list_push(&self, v: Value) -> Result<()> {
        self.with_list(|l| l.push(v))
    }

    /// Read a list element.
    pub fn list_get(&self, index: usize) -> Result<Value> {
        self.with_list_ref(|l| {
            l.get(index).cloned().ok_or(Error::IndexOutOfBounds {
                index,
                len: l.len(),
            })
        })?
    }

    /// Remove and return the last list element.
    pub fn list_pop(&self) -> Result<Value> {
        self.with_list(|l| l.pop().ok_or(Error::IndexOutOfBounds { index: 0, len: 0 }))?
    }

    /// Number of entries in a map.
    pub fn map_len(&self) -> Result<usize> {
        self.with_map_ref(|m| m.len())
    }

    /// Read a map entry.
    pub fn map_get(&self, key: &Value) -> Result<Value> {
        self.with_map_ref(|m| m.get(key).cloned().ok_or(Error::KeyNotFound))?
    }

    /// Write a map entry, returning the previous value if any.
    pub fn map_insert(&self, key: Value, value: Value) -> Result<Option<Value>> {
        self.with_map(|m| m.insert(key, value))
    }

    /// Remove a map entry.
    pub fn map_remove(&self, key: &Value) -> Result<Option<Value>> {
        self.with_map(|m| m.remove(key))
    }

    /// Set an object property, returning the previous own value if any.
    pub fn obj_set(&self, key: &str, value: Value) -> Result<Option<Value>> {
        self.with_object(|o| o.set(key, value))
    }

    /// Read an object property, following the prototype chain.
    ///
    /// # Errors
    ///
    /// [`Error::KeyNotFound`] only when the walk completed and found nothing.
    /// A temporarily unreadable prototype yields [`Error::BorrowConflict`], and
    /// a looping chain yields [`Error::ProtoCycle`], rather than being
    /// misreported as absence.
    pub fn obj_get(&self, key: &str) -> Result<Value> {
        self.with_object_ref(|o| o.get(key))?
            .and_then(|found| found.ok_or(Error::KeyNotFound))
    }

    /// Whether an object property is reachable through the prototype chain.
    pub fn obj_has(&self, key: &str) -> Result<bool> {
        self.with_object_ref(|o| o.has(key))?
    }

    /// An object value's prototype, as a shareable handle.
    pub fn proto(&self) -> Option<Gc<GcRefCell<Object>>> {
        self.as_object()?.try_borrow().ok()?.proto()
    }

    /// Attach `proto`, which must be an object value, as this object's
    /// prototype.
    pub fn obj_set_proto(&self, proto: &Value) -> Result<()> {
        let cell = match proto {
            Value::Object(o) => o,
            other => {
                return other.mismatch("Value::obj_set_proto", "object");
            }
        };
        self.with_object(|o| o.set_proto(cell.clone()))
    }

    /// Remove an object property.
    pub fn obj_remove(&self, key: &str) -> Result<Option<Value>> {
        self.with_object(|o| o.remove(key))
    }

    /// Call this value, threading an existing [`CallCtx`].
    ///
    /// Dispatches to a [`Value::Native`] body, or to an object's
    /// [`CALL_PROP`] property, inherited or overridden. Anything else is
    /// [`Error::NotCallable`].
    ///
    /// This is the only call entry point on purpose. An earlier `Value::call`
    /// that built a fresh [`CallCtx`] per invocation was trivially bypassable:
    /// a body re-entering through it got a depth counter of zero, so mutual
    /// recursion overflowed the native stack instead of returning
    /// [`Error::RecursionLimit`]. The counter only bounds recursion if every
    /// nested call goes through the same context.
    ///
    /// # Panics
    ///
    /// Never. Borrow conflicts and recursion limits are reported as errors, and
    /// the depth counter is unwound even if a body panics.
    pub fn call_with(&self, ctx: &CallCtx, args: &[Value]) -> Result<Value> {
        let callee = self.as_callable()?;
        let _guard = ctx.enter()?;
        callee.call(ctx, args)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Type(a), Value::Type(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            (Value::List(a), Value::List(b)) => Gc::ptr_eq(a, b),
            (Value::Map(a), Value::Map(b)) => Gc::ptr_eq(a, b),
            (Value::Object(a), Value::Object(b)) => Gc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => Gc::ptr_eq(a, b),
            (Value::User(a), Value::User(b)) => GcErased::ptr_eq(a, b),
            // Int and Float are deliberately NOT cross-compared. Comparing them
            // numerically is not transitive: Int(2^53+1) == Float(2^53) and
            // Int(2^53) == Float(2^53) but the two Ints differ, so `ValueMap`
            // would silently collapse three inserts into two and destroy a
            // value. Use `Value::as_float`, which widens an Int on read.
            _ => false,
        }
    }
}

impl Eq for Value {}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_at(f, 0)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_at(f, 0)
    }
}

impl Value {
    fn fmt_at(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        if depth > MAX_PRINT_DEPTH {
            return f.write_str("...");
        }
        match self {
            Value::Nil => f.write_str("nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Char(c) => write!(f, "{c:?}"),
            Value::Str(s) => write!(f, "{:?}", s.as_str()),
            Value::Bytes(b) => write!(f, "<{} bytes>", b.len()),
            Value::List(l) => match l.try_borrow() {
                Ok(items) => {
                    f.write_str("[")?;
                    for (i, v) in items.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        v.fmt_at(f, depth + 1)?;
                    }
                    f.write_str("]")
                }
                Err(_) => f.write_str("[<borrowed>]"),
            },
            Value::Map(m) => match m.try_borrow() {
                Ok(entries) => {
                    f.write_str("{")?;
                    for (i, (k, v)) in entries.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        k.fmt_at(f, depth + 1)?;
                        f.write_str(": ")?;
                        v.fmt_at(f, depth + 1)?;
                    }
                    f.write_str("}")
                }
                Err(_) => f.write_str("{<borrowed>}"),
            },
            Value::Object(o) => match o.try_borrow() {
                Ok(obj) => {
                    match obj.class() {
                        Some(c) => write!(f, "{c} {{")?,
                        None => f.write_str("{")?,
                    }
                    let keys = obj.keys();
                    for (i, k) in keys.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        f.write_str(k)?;
                        f.write_str(": ")?;
                        match obj.get(k) {
                            Ok(Some(v)) => v.fmt_at(f, depth + 1)?,
                            Ok(None) => f.write_str("<missing>")?,
                            Err(_) => f.write_str("<unreadable>")?,
                        }
                    }
                    f.write_str("}")
                }
                Err(_) => f.write_str("{<borrowed>}"),
            },
            Value::Native(n) => write!(f, "<native {}>", n.name()),
            Value::User(u) => write!(f, "<user {:?}>", u.type_id()),
            Value::Type(t) => write!(f, "<type {t:?}>"),
        }
    }
}
