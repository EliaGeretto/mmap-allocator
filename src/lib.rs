#![feature(allocator_api)]
#![feature(nonnull_slice_from_raw_parts)]
#![feature(slice_ptr_get)]
#![no_std]

use core::{
    alloc::{AllocError, Allocator, Layout},
    ffi::c_void,
    ptr::{self, NonNull},
};

#[derive(Clone, Copy, Default, Debug)]
pub struct MMapAllocator;

unsafe impl Allocator for MMapAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.align() > page_size::get() {
            // `mmap` can only map memory page-aligned.
            return Err(AllocError);
        }

        let layout = layout.align_to(page_size::get()).map_err(|_| AllocError)?;

        let new_mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                layout.size(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if new_mapping == libc::MAP_FAILED {
            return Err(AllocError);
        }

        // SAFETY: `mmap` is guaranteed to return a valid pointer if it
        // succeeds.
        let new_mapping = unsafe { NonNull::new_unchecked(new_mapping.cast::<u8>()) };

        Ok(NonNull::slice_from_raw_parts(
            new_mapping,
            layout.pad_to_align().size(),
        ))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // `ptr` is assumed to be currently allocated, thus the memory it points
        // to is currently mapped and also page-aligned.
        //
        // `layout.size()` fits the current memory block, so it always falls in
        // the last page of the current mapping.
        let res = libc::munmap(ptr.as_ptr().cast::<c_void>(), layout.size());
        if res == -1 {
            panic!("munmap failed");
        }
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // `mmap` always maps zeroed memory.
        self.allocate(layout)
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() >= old_layout.size(),
            "`new_layout.size()` must be greater than or equal to `old_layout.size()`"
        );

        if new_layout.align() > page_size::get() {
            // `mmap` can only map memory page-aligned.
            return Err(AllocError);
        }

        let old_layout = old_layout
            .align_to(page_size::get())
            .map_err(|_| AllocError)?;
        let new_layout = new_layout
            .align_to(page_size::get())
            .map_err(|_| AllocError)?;

        // When padded to alignment, `old_layout` gives the full size of the
        // previous allocation, so we check if there is enough space on the last
        // page to fit `new_layout`.
        if old_layout.pad_to_align() == new_layout.pad_to_align() {
            return Ok(NonNull::slice_from_raw_parts(
                ptr,
                new_layout.pad_to_align().size(),
            ));
        }

        let new_ptr = self.allocate(new_layout)?;

        // SAFETY: because `new_layout.size()` must be greater than or equal to
        // `old_layout.size()`, both the old and new memory allocation are valid for reads and
        // writes for `old_layout.size()` bytes. Also, because the old allocation wasn't yet
        // deallocated, it cannot overlap `new_ptr`. Thus, the call to `copy_nonoverlapping` is
        // safe. The safety contract for `dealloc` must be upheld by the caller.
        ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_layout.size());
        self.deallocate(ptr, old_layout);

        Ok(new_ptr)
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // When growing on the same page, the new memory area is not required to
        // be zeroed because it falls within the size returned for the old
        // allocation, which is always page-aligned.
        self.grow(ptr, old_layout, new_layout)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() <= old_layout.size(),
            "`new_layout.size()` must be smaller than or equal to `old_layout.size()`"
        );

        if new_layout.align() > page_size::get() {
            // `mmap` can only map memory page-aligned.
            return Err(AllocError);
        }

        let old_layout = old_layout
            .align_to(page_size::get())
            .map_err(|_| AllocError)?;
        let new_layout = new_layout
            .align_to(page_size::get())
            .map_err(|_| AllocError)?;

        // Unmap the pages at the end of the current mapping to avoid memory
        // leaks. The first portion of the current mapping can then just be
        // reused.

        let retained_area_size = new_layout.pad_to_align().size();
        let truncated_area_ptr = ptr.as_ptr().add(retained_area_size);
        let truncated_area_size = old_layout.pad_to_align().size() - retained_area_size;

        if truncated_area_size > 0 {
            let res = libc::munmap(truncated_area_ptr.cast::<c_void>(), truncated_area_size);
            if res == -1 {
                panic!("munmap failed");
            }
        }

        Ok(NonNull::slice_from_raw_parts(ptr, retained_area_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
