#![feature(allocator_api)]
#![feature(slice_ptr_get)]

use mmap_allocator::MMapAllocator;
use std::alloc::{Allocator, Layout};

fn allocate_deallocate(size: usize) -> usize {
    let allocator = MMapAllocator;

    let layout = Layout::from_size_align(size, 16).unwrap();
    let mut allocation = allocator.allocate(layout).expect("allocate failed");
    let allocation_slice = unsafe { allocation.as_mut() };

    let slice_size = allocation_slice.len();

    *allocation_slice.first_mut().unwrap() = 42;
    *allocation_slice.last_mut().unwrap() = 42;

    unsafe { allocator.deallocate(allocation.as_non_null_ptr(), layout) };

    slice_size
}

#[test]
fn allocate_deallocate_single_page() {
    let mapping_size = allocate_deallocate(10);

    // A small allocation should still be backed by a full page.
    assert_eq!(mapping_size, page_size::get());
}

#[test]
fn allocate_deallocate_multi_page() {
    let mapping_size = allocate_deallocate(page_size::get() + 10);

    // An allocation larger than one page should be backed by two pages
    assert_eq!(mapping_size, page_size::get() * 2);
}

#[test]
fn grow_inside_last_page() {
    let allocator = MMapAllocator;

    let initial_layout = Layout::from_size_align(10, 16).unwrap();
    let mut initial_allocation = allocator.allocate(initial_layout).expect("allocate failed");
    let allocation_slice = unsafe { initial_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), page_size::get());

    let grown_layout = Layout::from_size_align(16, 64).unwrap();
    let mut grown_allocation = unsafe {
        allocator
            .grow(
                initial_allocation.as_non_null_ptr(),
                initial_layout,
                grown_layout,
            )
            .expect("grow failed")
    };
    let allocation_slice = unsafe { grown_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), page_size::get()); // Size should not increase
    assert_eq!(initial_allocation.as_ptr(), grown_allocation.as_ptr()); // Pointer should be the same

    *allocation_slice.first_mut().unwrap() = 42;
    *allocation_slice.last_mut().unwrap() = 42;

    unsafe { allocator.deallocate(grown_allocation.as_non_null_ptr(), grown_layout) };
}

#[test]
fn grow_outside_last_page() {
    let allocator = MMapAllocator;

    let initial_layout = Layout::from_size_align(10, 16).unwrap();
    let mut initial_allocation = allocator.allocate(initial_layout).expect("allocate failed");
    let allocation_slice = unsafe { initial_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), page_size::get());

    *allocation_slice.first_mut().unwrap() = 42;

    let grown_layout = Layout::from_size_align(page_size::get() + 10, 64).unwrap();
    let mut grown_allocation = unsafe {
        allocator
            .grow(
                initial_allocation.as_non_null_ptr(),
                initial_layout,
                grown_layout,
            )
            .expect("grow failed")
    };
    let allocation_slice = unsafe { grown_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), 2 * page_size::get()); // The size should be double
    assert_ne!(initial_allocation.as_ptr(), grown_allocation.as_ptr()); // The map should be somewhere else

    // The data should be correctly transferred
    assert_eq!(*allocation_slice.first().unwrap(), 42);

    unsafe { allocator.deallocate(grown_allocation.as_non_null_ptr(), grown_layout) };
}

#[test]
fn shrink_inside_last_page() {
    let allocator = MMapAllocator;

    let initial_layout = Layout::from_size_align(page_size::get() + 16, 64).unwrap();
    let mut initial_allocation = allocator.allocate(initial_layout).expect("allocate failed");
    let allocation_slice = unsafe { initial_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), 2 * page_size::get());

    let shrunk_layout = Layout::from_size_align(page_size::get() + 10, 16).unwrap();
    let mut shrunk_allocation = unsafe {
        allocator
            .shrink(
                initial_allocation.as_non_null_ptr(),
                initial_layout,
                shrunk_layout,
            )
            .expect("shrink failed")
    };
    let allocation_slice = unsafe { shrunk_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), 2 * page_size::get()); // Size should not decrease
    assert_eq!(initial_allocation.as_ptr(), shrunk_allocation.as_ptr()); // Pointer should be the same

    unsafe { allocator.deallocate(shrunk_allocation.as_non_null_ptr(), shrunk_layout) };
}

#[test]
fn shrink_outside_last_page() {
    let allocator = MMapAllocator;

    let initial_layout = Layout::from_size_align(page_size::get() + 16, 64).unwrap();
    let mut initial_allocation = allocator.allocate(initial_layout).expect("allocate failed");
    let allocation_slice = unsafe { initial_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), 2 * page_size::get());

    let shrunk_layout = Layout::from_size_align(10, 16).unwrap();
    let mut shrunk_allocation = unsafe {
        allocator
            .shrink(
                initial_allocation.as_non_null_ptr(),
                initial_layout,
                shrunk_layout,
            )
            .expect("shrink failed")
    };
    let allocation_slice = unsafe { shrunk_allocation.as_mut() };
    assert_eq!(allocation_slice.len(), page_size::get()); // Size should shrink to one page
    assert_eq!(
        initial_allocation.as_mut_ptr(),
        shrunk_allocation.as_mut_ptr()
    ); // Pointer should remain the same

    unsafe { allocator.deallocate(shrunk_allocation.as_non_null_ptr(), shrunk_layout) };
}
