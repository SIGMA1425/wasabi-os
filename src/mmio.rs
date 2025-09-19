extern crate alloc;

use alloc::boxed::Box;
use core::mem::ManuallyDrop;
use core::pin::Pin;

pub struct Mmio<T: Sized> {
    inner: ManuallyDrop<Pin<Box<T>>>,
}

impl<T: Sized> Mmio<T> {
    /// # Safety
    pub unsafe fn from_raw(ptr: *mut T) -> Self {
        Self {
            inner: ManuallyDrop::new(Box::into_pin(Box::from_raw(ptr))),
        }
    }

    /// # Safety
    pub unsafe fn get_unchecked_mut(&mut self) -> &mut T {
        self.inner.as_mut().get_unchecked_mut()
    }
}

impl<T> AsRef<T> for Mmio<T> {
    fn as_ref(&self) -> &T {
        self.inner.as_ref().get_ref()
    }
}
