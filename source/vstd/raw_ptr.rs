#![allow(unused_imports)]

/*!
Tools and reasoning principles for [raw pointers](https://doc.rust-lang.org/std/primitive.pointer.html).  The tools here are meant to address "real Rust pointers, including all their subtleties on the Rust Abstract Machine, to the largest extent that is reasonable."

For a gentler introduction to some of the concepts here, see [`PPtr`](crate::simple_pptr), which uses a much-simplified pointer model.

### Pointer model

A pointer consists of an address (`ptr.addr()` or `ptr as usize`), a provenance `ptr@.provenance`,
and a `ptr@.metadata` (which is trivial except for pointers to non-sized types).
Note that in spec code, pointer equality requires *all 3* to be equal, whereas runtime equality (eq)
only compares addresses and metadata.

`*mut T` vs. `*const T` doesn't have any semantic different and Verus treats them as the same;
they can be seamlessly cast to and fro.
*/

use super::layout::*;
use super::prelude::*;

verus! {

//////////////////////////////////////
// Define a model of Ptrs and PointsTo
// Notes on mutability:
//
//  - Unique vs shared ownership in Verus is always determined
//    via the PointsTo ghost tracked object.
//
//  - Thus, there is effectively no difference between *mut T and *const T,
//    so we encode both of these in the same way.
//    (In VIR, we distinguish these via a decoration.)
//    Thus we can cast freely between them both in spec and exec code.
//
//  - This is consistent with Rust's operational semantics;
//    casting between *mut T and *const T has no operational meaning.
//
//  - When creating a pointer from a reference, the mutability
//    of the pointer *does* have an effect because it determines
//    what kind of "tag" the pointer gets, i.e., whether that
//    tag is readonly or not. In our model here, this tag is folded
//    into the provenance.
//
// Provenance:
//
//  - A full model of provenance is given by formalisms such as "Stacked Borrows"
//    or "Tree Borrows".
//
//  - None of these models are finalized, nor has Rust committed to then.
//    Rust's recent RFC on provenance simply details that there *is* some concept
//    of provenance.
//    https://rust-lang.github.io/rfcs/3559-rust-has-provenance.html
//
//  - Our model here, likewise, simply declares Provenance as an
//    abstract type.
//
//  - MiniRust currently declares a pointer has an Option<Provenance>;
//    the model here gives provenance a special "null" value instead
//    of using an option.
//
// More reading for reference:
//  - https://doc.rust-lang.org/std/ptr/
//  - https://github.com/minirust/minirust/tree/master
#[verifier::external_body]
pub ghost struct Provenance {}

impl Provenance {
    /// The provenance of the null ptr (or really, "no provenance")
    pub uninterp spec fn null() -> Self;
}

/// Metadata
///
/// For thin pointers (i.e., when T: Sized), the metadata is ()
/// For slices, str, and dyn types this is nontrivial
/// See: <https://doc.rust-lang.org/std/ptr/trait.Pointee.html>
///
/// TODO: This will eventually be replaced with `<T as Pointee>::Metadata`.
pub ghost enum Metadata {
    Thin,
    /// Length in bytes for a str; length in items for a
    Length(usize),
    /// For 'dyn' types (not yet supported)
    Dyn(DynMetadata),
}

#[verifier::external_body]
pub ghost struct DynMetadata {}

/// Model of a pointer `*mut T` or `*const T` in Rust's abstract machine
pub ghost struct PtrData {
    pub addr: usize,
    pub provenance: Provenance,
    pub metadata: Metadata,
}

/// Permission to access possibly-initialized, _typed_ memory.
// ptr |--> Init(v) means:
//   bytes in this memory are consistent with value v
//   and we have all the ghost state associated with type V
//
// ptr |--> Uninit means:
//   no knowledge about what's it memory
//   (to be pedantic, the bytes might be initialized in rust's abstract machine,
//   but we don't know so we have to pretend they're uninitialized)
#[verifier::external_body]
#[verifier::accept_recursive_types(T)]
pub tracked struct PointsTo<T> {
    phantom: core::marker::PhantomData<T>,
    no_copy: NoCopy,
}

//#[verifier::external_body]
//#[verifier::accept_recursive_types(T)]
//pub tracked struct PointsToBytes<T> {
//    phantom: core::marker::PhantomData<T>,
//    no_copy: NoCopy,
//}
/// Represents (typed) contents of memory.
// Don't use std Option here in order to avoid circular dependency issues
// with verifying the standard library.
// (Also, using our own enum here lets us have more meaningful
// variant names like Uninit/Init.)
#[verifier::accept_recursive_types(T)]
pub ghost enum MemContents<T> {
    /// Represents uninitialized memory
    Uninit,
    /// Represents initialized memory with the given value
    Init(T),
}

pub ghost struct PointsToData<T> {
    pub ptr: *mut T,
    pub opt_value: MemContents<T>,
}

//pub ghost struct PointsToBytesData<T> {
//    pub provena
//}
impl<T: ?Sized> View for *mut T {
    type V = PtrData;

    uninterp spec fn view(&self) -> Self::V;
}

impl<T: ?Sized> View for *const T {
    type V = PtrData;

    #[verifier::inline]
    open spec fn view(&self) -> Self::V {
        (*self as *mut T).view()
    }
}

impl<T> View for PointsTo<T> {
    type V = PointsToData<T>;

    uninterp spec fn view(&self) -> Self::V;
}

impl<T> PointsTo<T> {
    #[verifier::inline]
    pub open spec fn ptr(&self) -> *mut T {
        self.view().ptr
    }

    #[verifier::inline]
    pub open spec fn opt_value(&self) -> MemContents<T> {
        self.view().opt_value
    }

    #[verifier::inline]
    pub open spec fn is_init(&self) -> bool {
        self.opt_value().is_init()
    }

    #[verifier::inline]
    pub open spec fn is_uninit(&self) -> bool {
        self.opt_value().is_uninit()
    }

    #[verifier::inline]
    pub open spec fn value(&self) -> T {
        self.opt_value().value()
    }

    /// Guarantee that the `PointsTo` for any non-zero-sized type points to a non-null address.
    ///
    // ZST pointers *are* allowed to be null, so we need a precondition that size != 0.
    // See https://doc.rust-lang.org/std/ptr/#safety
    pub axiom fn is_nonnull(tracked &self)
        requires
            size_of::<T>() != 0,
        ensures
            self@.ptr@.addr != 0,
    ;

    /// "Forgets" about the value stored behind the pointer.
    /// Updates the `PointsTo` value to [`MemContents::Uninit`](MemContents::Uninit).
    /// Note that this is a `proof` function, i.e.,
    /// it is operationally a no-op in executable code, even on the Rust Abstract Machine.
    /// Only the proof-code representation changes.
    pub axiom fn leak_contents(tracked &mut self)
        ensures
            self.ptr() == old(self).ptr(),
            self.is_uninit(),
    ;

    /// Note: If both S and T are non-zero-sized, then this implies the pointers
    /// have distinct addresses.
    pub axiom fn is_disjoint<S>(tracked &mut self, tracked other: &PointsTo<S>)
        ensures
            *old(self) == *self,
            self.ptr() as int + size_of::<T>() <= other.ptr() as int || other.ptr() as int
                + size_of::<S>() <= self.ptr() as int,
    ;
}

impl<T> MemContents<T> {
    #[verifier::inline]
    pub open spec fn is_init(&self) -> bool {
        self is Init
    }

    #[verifier::inline]
    pub open spec fn is_uninit(&self) -> bool {
        self is Uninit
    }

    #[verifier::inline]
    pub open spec fn value(&self) -> T {
        self->0
    }
}

//////////////////////////////////////
// Inverse functions:
// Pointers are equivalent to their model
pub uninterp spec fn ptr_mut_from_data<T: ?Sized>(data: PtrData) -> *mut T;

#[verifier::inline]
pub open spec fn ptr_from_data<T: ?Sized>(data: PtrData) -> *const T {
    ptr_mut_from_data(data) as *const T
}

pub broadcast axiom fn axiom_ptr_mut_from_data<T: ?Sized>(data: PtrData)
    ensures
        (#[trigger] ptr_mut_from_data::<T>(data))@ == data,
;

// Equiv to ptr_mut_from_data, but named differently to avoid trigger issues
// Only use for ptrs_mut_eq
#[doc(hidden)]
pub uninterp spec fn view_reverse_for_eq<T: ?Sized>(data: PtrData) -> *mut T;

/// Implies that `a@ == b@ ==> a == b`.
pub broadcast axiom fn ptrs_mut_eq<T: ?Sized>(a: *mut T)
    ensures
        view_reverse_for_eq::<T>(#[trigger] a@) == a,
;

//////////////////////////////////////
// Null ptrs
// NOTE: trait aliases are not yet supported,
// so we use Pointee<Metadata = ()> instead of core::ptr::Thin here
#[verifier::inline]
pub open spec fn ptr_null<T: ?Sized + core::ptr::Pointee<Metadata = ()>>() -> *const T {
    ptr_from_data(PtrData { addr: 0, provenance: Provenance::null(), metadata: Metadata::Thin })
}

#[cfg(verus_keep_ghost)]
#[verifier::when_used_as_spec(ptr_null)]
pub assume_specification<
    T: ?Sized + core::ptr::Pointee<Metadata = ()>,
>[ core::ptr::null ]() -> (res: *const T)
    ensures
        res == ptr_null::<T>(),
    opens_invariants none
    no_unwind
;

#[verifier::inline]
pub open spec fn ptr_null_mut<T: ?Sized + core::ptr::Pointee<Metadata = ()>>() -> *mut T {
    ptr_mut_from_data(PtrData { addr: 0, provenance: Provenance::null(), metadata: Metadata::Thin })
}

#[cfg(verus_keep_ghost)]
#[verifier::when_used_as_spec(ptr_null_mut)]
pub assume_specification<
    T: ?Sized + core::ptr::Pointee<Metadata = ()>,
>[ core::ptr::null_mut ]() -> (res: *mut T)
    ensures
        res == ptr_null_mut::<T>(),
    opens_invariants none
    no_unwind
;

//////////////////////////////////////
// Casting
// as-casts and implicit casts are translated internally to these functions
// (including casts that involve *const ptrs)
pub open spec fn spec_cast_ptr_to_thin_ptr<T: ?Sized, U: Sized>(ptr: *mut T) -> *mut U {
    ptr_mut_from_data(
        PtrData { addr: ptr@.addr, provenance: ptr@.provenance, metadata: Metadata::Thin },
    )
}

/// Don't call this directly; use an `as`-cast instead.
#[verifier::external_body]
#[cfg_attr(verus_keep_ghost, rustc_diagnostic_item = "verus::vstd::raw_ptr::cast_ptr_to_thin_ptr")]
#[verifier::when_used_as_spec(spec_cast_ptr_to_thin_ptr)]
pub fn cast_ptr_to_thin_ptr<T: ?Sized, U: Sized>(ptr: *mut T) -> (result: *mut U)
    ensures
        result == spec_cast_ptr_to_thin_ptr::<T, U>(ptr),
    opens_invariants none
    no_unwind
{
    ptr as *mut U
}

pub open spec fn spec_cast_array_ptr_to_slice_ptr<T, const N: usize>(ptr: *mut [T; N]) -> *mut [T] {
    ptr_mut_from_data(
        PtrData { addr: ptr@.addr, provenance: ptr@.provenance, metadata: Metadata::Length(N) },
    )
}

/// Don't call this directly; use an `as`-cast instead.
#[verifier::external_body]
#[cfg_attr(verus_keep_ghost, rustc_diagnostic_item = "verus::vstd::raw_ptr::cast_array_ptr_to_slice_ptr")]
#[verifier::when_used_as_spec(spec_cast_array_ptr_to_slice_ptr)]
pub fn cast_array_ptr_to_slice_ptr<T, const N: usize>(ptr: *mut [T; N]) -> (result: *mut [T])
    ensures
        result == spec_cast_array_ptr_to_slice_ptr(ptr),
    opens_invariants none
    no_unwind
{
    ptr as *mut [T]
}

pub open spec fn spec_cast_ptr_to_usize<T: Sized>(ptr: *mut T) -> usize {
    ptr@.addr
}

/// Don't call this directly; use an `as`-cast instead.
#[verifier::external_body]
#[cfg_attr(verus_keep_ghost, rustc_diagnostic_item = "verus::vstd::raw_ptr::cast_ptr_to_usize")]
#[verifier::when_used_as_spec(spec_cast_ptr_to_usize)]
pub fn cast_ptr_to_usize<T: Sized>(ptr: *mut T) -> (result: usize)
    ensures
        result == spec_cast_ptr_to_usize(ptr),
    opens_invariants none
    no_unwind
{
    ptr as usize
}

//////////////////////////////////////
// Reading and writing
/// Calls `core::ptr::write`
///
/// This does _not_ drop the contents. If the memory is already initialized and you want
/// to write without dropping, call [`PointsTo::leak_contents`] first.
#[inline(always)]
#[verifier::external_body]
pub fn ptr_mut_write<T>(ptr: *mut T, Tracked(perm): Tracked<&mut PointsTo<T>>, v: T)
    requires
        old(perm).ptr() == ptr,
        old(perm).is_uninit(),
    ensures
        perm.ptr() == ptr,
        perm.opt_value() == MemContents::Init(v),
    opens_invariants none
    no_unwind
{
    unsafe {
        core::ptr::write(ptr, v);
    }
}

/// core::ptr::read
///
/// This leaves the data as "unitialized", i.e., performs a move.
///
/// TODO This needs to be made more general (i.e., should be able to read a Copy type
/// without destroying it; should be able to leave the bytes intact without uninitializing them)
#[inline(always)]
#[verifier::external_body]
pub fn ptr_mut_read<T>(ptr: *const T, Tracked(perm): Tracked<&mut PointsTo<T>>) -> (v: T)
    requires
        old(perm).ptr() == ptr,
        old(perm).is_init(),
    ensures
        perm.ptr() == ptr,
        perm.is_uninit(),
        v == old(perm).value(),
    opens_invariants none
    no_unwind
{
    unsafe { core::ptr::read(ptr) }
}

/// equivalent to &*X
#[inline(always)]
#[verifier::external_body]
pub fn ptr_ref<T>(ptr: *const T, Tracked(perm): Tracked<&PointsTo<T>>) -> (v: &T)
    requires
        perm.ptr() == ptr,
        perm.is_init(),
    ensures
        v == perm.value(),
    opens_invariants none
    no_unwind
{
    unsafe { &*ptr }
}

/* coming soon
/// equivalent to &mut *X
#[inline(always)]
#[verifier::external_body]
pub fn ptr_mut_ref<T>(ptr: *mut T, Tracked(perm): Tracked<&mut PointsTo<T>>) -> (v: &mut T)
    requires
        old(perm).ptr() == ptr,
        old(perm).is_init()
    ensures
        perm.ptr() == ptr,
        perm.is_init(),

        old(perm).value() == *old(v),
        new(perm).value() == *new(v),
    unsafe { &*ptr }
}
*/

macro_rules! pointer_specs {
    ($mod_ident:ident, $ptr_from_data:ident, $mu:tt) => {
        #[cfg(verus_keep_ghost)]
        mod $mod_ident {
            use super::*;

            verus!{

            #[verifier::inline]
            pub open spec fn spec_addr<T: ?Sized>(p: *$mu T) -> usize { p@.addr }

            #[verifier::when_used_as_spec(spec_addr)]
            pub assume_specification<T: ?Sized>[<*$mu T>::addr](p: *$mu T) -> (addr: usize)
                ensures addr == spec_addr(p)
                opens_invariants none
                no_unwind;

            pub open spec fn spec_with_addr<T: ?Sized>(p: *$mu T, addr: usize) -> *$mu T {
                $ptr_from_data(PtrData { addr: addr, .. p@ })
            }

            #[verifier::when_used_as_spec(spec_with_addr)]
            pub assume_specification<T: ?Sized>[<*$mu T>::with_addr](p: *$mu T, addr: usize) -> (q: *$mu T)
                ensures q == spec_with_addr(p, addr)
                opens_invariants none
                no_unwind;

            }
        }
    };
}

pointer_specs!(ptr_mut_specs, ptr_mut_from_data, mut);

pointer_specs!(ptr_const_specs, ptr_from_data, const);

pub broadcast group group_raw_ptr_axioms {
    axiom_ptr_mut_from_data,
    ptrs_mut_eq,
}

// Exposing provenance
#[verifier::external_body]
pub tracked struct IsExposed {}

impl Clone for IsExposed {
    #[verifier::external_body]
    fn clone(&self) -> (s: Self)
        ensures
            s == self,
    {
        IsExposed {  }
    }
}

impl Copy for IsExposed {

}

impl IsExposed {
    pub open spec fn view(self) -> Provenance {
        self.provenance()
    }

    pub uninterp spec fn provenance(self) -> Provenance;

    pub axiom fn null() -> (tracked exp: IsExposed)
        ensures
            exp.provenance() == Provenance::null(),
    ;
}

/// Perform a provenance expose operation.
#[verifier::external_body]
pub fn expose_provenance<T: Sized>(m: *mut T) -> (provenance: Tracked<IsExposed>)
    ensures
        provenance@@ == m@.provenance,
    opens_invariants none
    no_unwind
{
    let _ = m as usize;
    Tracked::assume_new()
}

/// Construct a pointer with the given provenance from a _usize_ address.
/// The provenance must have previously been exposed.
#[verifier::external_body]
pub fn with_exposed_provenance<T: Sized>(
    addr: usize,
    Tracked(provenance): Tracked<IsExposed>,
) -> (p: *mut T)
    ensures
        p == ptr_mut_from_data::<T>(
            PtrData { addr: addr, provenance: provenance@, metadata: Metadata::Thin },
        ),
    opens_invariants none
    no_unwind
{
    addr as *mut T
}

/// PointsToRaw
/// Variable-sized uninitialized memory.
///
/// Permission is for an arbitrary set of addresses, not necessarily contiguous,
/// and with a given provenance.
// Note reading from uninitialized memory is UB, so we shouldn't give any
// reading capabilities to PointsToRaw. Turning a PointsToRaw into a PointsTo
// should always leave it as 'uninitialized'.
#[verifier::external_body]
pub tracked struct PointsToRaw {
    // TODO implement this as Map<usize, PointsTo<u8>> or something
    no_copy: NoCopy,
}

impl PointsToRaw {
    pub uninterp spec fn provenance(self) -> Provenance;

    pub uninterp spec fn dom(self) -> Set<int>;

    pub open spec fn is_range(self, start: int, len: int) -> bool {
        super::set_lib::set_int_range(start, start + len) =~= self.dom()
    }

    pub open spec fn contains_range(self, start: int, len: int) -> bool {
        super::set_lib::set_int_range(start, start + len) <= self.dom()
    }

    pub axiom fn empty(provenance: Provenance) -> (tracked points_to_raw: Self)
        ensures
            points_to_raw.dom() == Set::<int>::empty(),
            points_to_raw.provenance() == provenance,
    ;

    pub axiom fn split(tracked self, range: Set<int>) -> (tracked res: (Self, Self))
        requires
            range.subset_of(self.dom()),
        ensures
            res.0.provenance() == self.provenance(),
            res.1.provenance() == self.provenance(),
            res.0.dom() == range,
            res.1.dom() == self.dom().difference(range),
    ;

    pub axiom fn join(tracked self, tracked other: Self) -> (tracked joined: Self)
        requires
            self.provenance() == other.provenance(),
        ensures
            joined.provenance() == self.provenance(),
            joined.dom() == self.dom() + other.dom(),
    ;

    // In combination with PointsToRaw::empty(),
    // This lets us create a PointsTo for a ZST for _any_ pointer (any address and provenance).
    // (even null).
    // Admittedly, this does violate 'strict provenance';
    // https://doc.rust-lang.org/std/ptr/#using-strict-provenance)
    // but that's ok. It is still allowed in Rust's more permissive semantics.
    pub axiom fn into_typed<V>(tracked self, start: usize) -> (tracked points_to: PointsTo<V>)
        requires
            start as int % align_of::<V>() as int == 0,
            self.is_range(start as int, size_of::<V>() as int),
        ensures
            points_to.ptr() == ptr_mut_from_data::<V>(
                PtrData { addr: start, provenance: self.provenance(), metadata: Metadata::Thin },
            ),
            points_to.is_uninit(),
    ;
}

impl<V> PointsTo<V> {
    pub axiom fn into_raw(tracked self) -> (tracked points_to_raw: PointsToRaw)
        requires
            self.is_uninit(),
        ensures
            points_to_raw.is_range(self.ptr().addr() as int, size_of::<V>() as int),
            points_to_raw.provenance() == self.ptr()@.provenance,
    ;
}

// Allocation and deallocation via the global allocator
/// Permission to perform a deallocation with the global allocator
#[verifier::external_body]
pub tracked struct Dealloc {
    no_copy: NoCopy,
}

pub ghost struct DeallocData {
    pub addr: usize,
    pub size: nat,
    pub align: nat,
    /// This should probably be some kind of "allocation ID" (with "allocation ID" being
    /// only one part of a full Provenance definition).
    pub provenance: Provenance,
}

impl Dealloc {
    pub uninterp spec fn view(self) -> DeallocData;

    #[verifier::inline]
    pub open spec fn addr(self) -> usize {
        self.view().addr
    }

    #[verifier::inline]
    pub open spec fn size(self) -> nat {
        self.view().size
    }

    #[verifier::inline]
    pub open spec fn align(self) -> nat {
        self.view().align
    }

    #[verifier::inline]
    pub open spec fn provenance(self) -> Provenance {
        self.view().provenance
    }
}

/// Allocate with the global allocator.
/// Precondition should be consistent with the [documented safety conditions on `alloc`](https://doc.rust-lang.org/alloc/alloc/trait.GlobalAlloc.html#tymethod.alloc).
#[cfg(feature = "std")]
#[verifier::external_body]
pub fn allocate(size: usize, align: usize) -> (pt: (
    *mut u8,
    Tracked<PointsToRaw>,
    Tracked<Dealloc>,
))
    requires
        valid_layout(size, align),
        size != 0,
    ensures
        pt.1@.is_range(pt.0.addr() as int, size as int),
        pt.2@@ == (DeallocData {
            addr: pt.0.addr(),
            size: size as nat,
            align: align as nat,
            provenance: pt.1@.provenance(),
        }),
        pt.0.addr() as int % align as int == 0,
        pt.0@.metadata == Metadata::Thin,
        pt.0@.provenance == pt.1@.provenance(),
    opens_invariants none
{
    // SAFETY: valid_layout is a precondition
    let layout = unsafe { alloc::alloc::Layout::from_size_align_unchecked(size, align) };
    // SAFETY: size != 0
    let p = unsafe { ::alloc::alloc::alloc(layout) };
    if p == core::ptr::null_mut() {
        std::process::abort();
    }
    (p, Tracked::assume_new(), Tracked::assume_new())
}

/// Deallocate with the global allocator.
#[cfg(feature = "alloc")]
#[verifier::external_body]
pub fn deallocate(
    p: *mut u8,
    size: usize,
    align: usize,
    Tracked(pt): Tracked<PointsToRaw>,
    Tracked(dealloc): Tracked<Dealloc>,
)
    requires
        dealloc.addr() == p.addr(),
        dealloc.size() == size,
        dealloc.align() == align,
        dealloc.provenance() == pt.provenance(),
        pt.is_range(dealloc.addr() as int, dealloc.size() as int),
        p@.provenance == dealloc.provenance(),
    opens_invariants none
{
    // SAFETY: ensured by dealloc token
    let layout = unsafe { alloc::alloc::Layout::from_size_align_unchecked(size, align) };
    unsafe {
        ::alloc::alloc::dealloc(p, layout);
    }
}

/// This is meant to be a replacement for `&'a T` that allows Verus to keep track of
/// not just the `T` value but the pointer as well.
/// It would be better to get rid of this and use normal reference types `&'a T`,
/// but there are a lot of unsolved implementation questions.
/// The existence of `SharedReference<'a, T>` is a stop-gap.
#[verifier::external_body]
#[verifier::accept_recursive_types(T)]
pub struct SharedReference<'a, T>(&'a T);

impl<'a, T> Clone for SharedReference<'a, T> {
    #[verifier::external_body]
    fn clone(&self) -> (ret: Self)
        ensures
            ret == *self,
    {
        SharedReference(self.0)
    }
}

impl<'a, T> Copy for SharedReference<'a, T> {

}

impl<'a, T> SharedReference<'a, T> {
    pub uninterp spec fn value(self) -> T;

    pub uninterp spec fn ptr(self) -> *const T;

    #[verifier::external_body]
    fn new(t: &'a T) -> (s: Self)
        ensures
            s.value() == t,
    {
        SharedReference(t)
    }

    #[verifier::external_body]
    fn as_ref(self) -> (t: &'a T)
        ensures
            t == self.value(),
    {
        self.0
    }

    #[verifier::external_body]
    fn as_ptr(self) -> (ptr: *const T)
        ensures
            ptr == self.ptr(),
    {
        &*self.0
    }

    axiom fn points_to(tracked self) -> (tracked pt: &'a PointsTo<T>)
        ensures
            pt.ptr() == self.ptr(),
            pt.is_init(),
            pt.value() == self.value(),
    ;
}

/// Like [`ptr_ref`] but returns a `SharedReference` so it keeps track of the relationship
/// between the pointers.
/// Note the resulting reference's pointers does NOT have the same provenance.
/// This is because in Rust models like Stacked Borrows / Tree Borrows, the pointer
/// gets a new tag.
#[inline(always)]
#[verifier::external_body]
pub fn ptr_ref2<'a, T>(ptr: *const T, Tracked(perm): Tracked<&PointsTo<T>>) -> (v: SharedReference<
    'a,
    T,
>)
    requires
        perm.ptr() == ptr,
        perm.is_init(),
    ensures
        v.value() == perm.value(),
        v.ptr().addr() == ptr.addr(),
        v.ptr()@.metadata == ptr@.metadata,
    opens_invariants none
    no_unwind
{
    SharedReference(unsafe { &*ptr })
}

} // verus!
