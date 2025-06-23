extern crate alloc;

use crate::result::Result;
use crate::serial::SerialPort;
use crate::uefi::EfiMemoryDescriptor;
use crate::uefi::EfiMemoryType;
use crate::uefi::MemoryMapHolder;

use alloc::alloc::GlobalAlloc;
use alloc::alloc::Layout;
use alloc::boxed::Box;

use core::borrow::BorrowMut;
use core::cell::RefCell;
use core::cmp::max;
use core::fmt;
use core::fmt::Write;
use core::mem::size_of;
use core::ops::DerefMut;
use core::ptr::null_mut;

pub fn round_up_to_nearest_pow2(v: usize) -> Result<usize> {
    1usize
        .checked_shl(usize::BITS - v.wrapping_sub(1).leading_zeros())
        .ok_or("Out of range")
}

// 次のヘッダのアドレス、この領域のサイズ
struct Header {
    next_header: Option<Box<Header>>,
    size: usize,
    is_allocated: bool,
    _reserved: usize,
}

// 32bit メモリの情報とその領域のサイズを確保するためのサイズ
const HEADER_SIZE: usize = size_of::<Header>();

// HEADER_SIZEが32bitになっているかのチェック
#[allow(clippy::assertions_on_constants)]
const _: () = assert!(HEADER_SIZE == 32);
const _: () = assert!(HEADER_SIZE.count_ones() == 1);

pub const LAYOUT_PAGE_4K: Layout = unsafe { Layout::from_size_align_unchecked(4096, 4096) };

impl Header {
    fn can_provide(&self, size: usize, align: usize) -> bool {
        self.size >= size + HEADER_SIZE * 2 * align
    }
    fn is_allocated(&self) -> bool {
        self.is_allocated
    }
    fn end_addr(&self) -> usize {
        self as *const Header as usize + self.size
    }
    // アドレスからヘッダを作成
    unsafe fn new_from_addr(addr: usize) -> Box<Header> {
        let header = addr as *mut Header;
        header.write(Header {
            next_header: None,
            size: 0,
            is_allocated: false,
            _reserved: 0,
        });
        Box::from_raw(addr as *mut Header)
    }
    unsafe fn from_allocated_region(addr: *mut u8) -> Box<Header> {
        let header = addr.sub(HEADER_SIZE) as *mut Header;
        Box::from_raw(header)
    }
    // 要求されている大きさとアライメントを満たすメモリ領域を空き領域から切り出すことを試みる
    // 切り出せない場合はNone
    // 切り出せた場合はそのアドレスをSomeで返す
    fn provide(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        // sizeとalignの調整
        // HEADER_SIZEより小さければこれに修正
        // 2のべき乗に切り上げ
        let size = max(round_up_to_nearest_pow2(size).ok()?, HEADER_SIZE);
        let align = max(align, HEADER_SIZE);

        // 要求された領域を切り出す
        if self.is_allocated() || !self.can_provide(size, align) {
            None
        } else {
            // 使われているさいず
            let mut size_used = 0;
            // 割り当てられているアドレス
            let allocated_addr = (self.end_addr() - size) & !(align - 1);
            let mut header_for_allocated =
                unsafe { Self::new_from_addr(allocated_addr - HEADER_SIZE) };
            header_for_allocated.is_allocated = true;
            header_for_allocated.size = size + HEADER_SIZE;
            size_used += header_for_allocated.size;
            header_for_allocated.next_header = self.next_header.take();

            if header_for_allocated.end_addr() != self.end_addr() {
                let mut header_for_padding =
                    unsafe { Self::new_from_addr(header_for_allocated.end_addr()) };
                header_for_padding.is_allocated = false;
                header_for_padding.size = self.end_addr() - header_for_allocated.end_addr();
                size_used += header_for_padding.size;
                header_for_padding.next_header = header_for_allocated.next_header.take();
                header_for_allocated.next_header = Some(header_for_padding);
            }

            assert!(self.size >= size_used + HEADER_SIZE);
            self.size -= size_used;
            self.next_header = Some(header_for_allocated);
            Some(allocated_addr as *mut u8)
        }
    }
}

impl Drop for Header {
    fn drop(&mut self) {
        panic!("Header should not be dropped!");
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write! {
            f,
            "Header @ {:#018X} {{size: {:#018X}, is_allocated: {} }}",
            self as *const Header as usize,
            self.size,
            self.is_allocated()
        }
    }
}

#[test_case]
fn round_up_to_nearest_pow2_tests() {
    // unimplemented!("Cargo test should fail, right...?")
    assert_eq!(round_up_to_nearest_pow2(0), Err("Out of range"));
    assert_eq!(round_up_to_nearest_pow2(1), Ok(1));
    assert_eq!(round_up_to_nearest_pow2(2), Ok(2));
    assert_eq!(round_up_to_nearest_pow2(3), Ok(4));
    assert_eq!(round_up_to_nearest_pow2(4), Ok(4));
    assert_eq!(round_up_to_nearest_pow2(5), Ok(8));
    assert_eq!(round_up_to_nearest_pow2(6), Ok(8));
    assert_eq!(round_up_to_nearest_pow2(7), Ok(8));
    assert_eq!(round_up_to_nearest_pow2(8), Ok(8));
    assert_eq!(round_up_to_nearest_pow2(9), Ok(16));
}

// アロケータの本体
pub struct FirstFitAllocator {
    first_header: RefCell<Option<Box<Header>>>,
}

// FirstFitAllocatorのインスタンス
// global_allocator: Rustのallocのクレートがこれを使うようになる
#[global_allocator]
pub static ALLOCATOR: FirstFitAllocator = FirstFitAllocator {
    first_header: RefCell::new(None),
};

unsafe impl Sync for FirstFitAllocator {}

unsafe impl GlobalAlloc for FirstFitAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_with_options(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let mut region = Header::from_allocated_region(ptr);
        region.is_allocated = false;
        Box::leak(region);
    }
}

impl FirstFitAllocator {
    //  メモリアロケータの処理の本体
    pub fn alloc_with_options(&self, layout: Layout) -> *mut u8 {
        let mut header = self.first_header.borrow_mut();
        let mut header = header.deref_mut();

        // 空き領域のリストを順に見て、provideを呼び出す
        // メモリが確保できたら、そのアドレスを返す
        // メモリが確保できなければNULL
        loop {
            match header {
                Some(e) => {
                    match e.provide(layout.size(), layout.align()) {
                        Some(p) => break p,
                        None => {
                            header = e.next_header.borrow_mut();
                            continue;
                        }
                    }
                }
                None => {
                    break null_mut::<u8>();
                }
            }
        }
    }

    // UEFIからのメモリマップからの初期化
    pub fn init_with_mmap(&self, memory_map: &MemoryMapHolder) {
        for e in memory_map.iter() {
            if e.memory_type() != EfiMemoryType::CONVENTIONAL_MEMORY {
                continue;
            }
            self.add_free_from_descriptor(e);
        }
    }

    // Descriptorから空き領域を追加
    fn add_free_from_descriptor(&self, desc: &EfiMemoryDescriptor) {
        let mut start_addr = desc.physical_start() as usize;
        let mut size = desc.number_of_pages() as usize * 4096;
        if start_addr == 0 {
            start_addr = 4096;
            size = size.saturating_sub(4096);
        }
        if size <= 4096 {
            return;
        }

        // Headerの作成
        let mut header = unsafe { Header::new_from_addr(start_addr) };
        header.next_header = None;
        header.is_allocated = false;
        header.size = size;

        // 現在の最初のHeader
        let mut first_header = self.first_header.borrow_mut();
        // さっき作った現在の先頭Headerをprev_lastに
        // first_headerはheaderに置き換え
        let prev_last = first_header.replace(header);
        // first_headerの借用を削除
        drop(first_header);

        // さっき作ったheader
        // first_headr.replace(self.first_headerの借用)を置き換えているのでheaderになっている
        let mut header = self.first_header.borrow_mut();
        // headerのnextにさっきまでの先頭Headerを連結
        header.as_mut().unwrap().next_header = prev_last;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn malloc_iterate_free_and_alloc() {
        use alloc::vec::Vec;
        for i in 0..1000 {
            let mut vec = Vec::new();
            vec.resize(i, 10);
        }
    }

    #[test_case]
    fn malloc_align() {
        let mut pointers = [null_mut::<u8>(); 100];
        for align in [1, 2, 4, 8, 16, 32, 4096] {
            for e in pointers.iter_mut() {
                *e = ALLOCATOR.alloc_with_options(
                    Layout::from_size_align(1234, align).expect("Failed to create Layout!!!"),
                );
                assert!(*e as usize != 0);
                assert!((*e as usize) % align == 0);
            }
        }
    }

    #[test_case]
    fn malloc_align_random_order() {
        for align in [32, 4096, 8, 4, 16, 2, 1] {
            let mut pointers = [null_mut::<u8>(); 100];
            for e in pointers.iter_mut() {
                *e = ALLOCATOR.alloc_with_options(
                    Layout::from_size_align(1234, align).expect("Failed to create Layout"),
                );
                assert!(*e as usize != 0);
                assert!((*e as usize) % align == 0);
            }
        }
    }

    #[test_case]
    fn allocated_objects_have_no_overlap() {
        let allocations = [
            Layout::from_size_align(128, 128).unwrap(),
            Layout::from_size_align(32, 32).unwrap(),
            Layout::from_size_align(8, 8).unwrap(),
            Layout::from_size_align(16, 16).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(4, 4).unwrap(),
            Layout::from_size_align(2, 2).unwrap(),
            Layout::from_size_align(600000, 64).unwrap(),
            Layout::from_size_align(64, 64).unwrap(),
            Layout::from_size_align(1, 1).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(3, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(600000, 64).unwrap(),
            Layout::from_size_align(6000, 64).unwrap(),
            Layout::from_size_align(60000, 64).unwrap(),
            Layout::from_size_align(60000, 64).unwrap(),
            Layout::from_size_align(60000, 64).unwrap(),
            Layout::from_size_align(60000, 64).unwrap(),
        ];
        let mut pointers = vec![null_mut::<u8>(); allocations.len()];
        for e in allocations.iter().zip(pointers.iter_mut()).enumerate() {
            let (i, (layout, pointer)) = e;
            *pointer = ALLOCATOR.alloc_with_options(*layout);
            for k in 0..layout.size() {
                unsafe { *pointer.add(k) = i as u8 }
            }
        }
        for e in allocations.iter().zip(pointers.iter_mut()).enumerate() {
            let (i, (layout, pointer)) = e;
            for k in 0..layout.size() {
                assert!(unsafe { *pointer.add(k) } == i as u8);
            }
        }
        for e in allocations
            .iter()
            .zip(pointers.iter_mut())
            .enumerate()
            .step_by(2)
        {
            let (i, (layout, pointer)) = e;
            for k in 0..layout.size() {
                assert!(unsafe { *pointer.add(k) } == i as u8);
            }
        }
        for e in allocations
            .iter()
            .zip(pointers.iter_mut())
            .enumerate()
            .step_by(2)
        {
            let (i, (layout, pointer)) = e;
            *pointer = ALLOCATOR.alloc_with_options(*layout);
            for k in 0..layout.size() {
                unsafe { *pointer.add(k) = i as u8 }
            }
        }
        for e in allocations.iter().zip(pointers.iter_mut()).enumerate() {
            let (i, (layout, pointer)) = e;
            for k in 0..layout.size() {
                assert!(unsafe { *pointer.add(k) } == i as u8);
            }
        }
    }
}
