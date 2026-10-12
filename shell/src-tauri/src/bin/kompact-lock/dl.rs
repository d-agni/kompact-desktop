//! 14a: libraries loaded when kompact-lock starts (not linked), so the shell builds without
//! them: libgtk-session-lock (`sudo apt install libgtk-session-lock0`) and libpam.

use std::ffi::{c_void, CStr, CString};

pub struct Lib(*mut c_void);

// SAFETY: a dlopen handle is process-wide and never closed.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

impl Lib {
    pub fn open(name: &str) -> Result<Lib, String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: a valid C string; the handle is kept for the process's life.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if h.is_null() {
            // SAFETY: dlerror returns a static C string or null.
            let err = unsafe { libc::dlerror() };
            let why = if err.is_null() { "not found".to_string() } else { unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned() };
            return Err(format!("can't load {name}: {why}"));
        }
        Ok(Lib(h))
    }

    /// The symbol `name` as a function pointer of type `F`.
    ///
    /// # Safety
    /// `F` must be the symbol's real C signature (an `unsafe extern "C" fn`).
    pub unsafe fn get<F: Copy>(&self, name: &str) -> Result<F, String> {
        assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
        let c = CString::new(name).map_err(|e| e.to_string())?;
        let p = libc::dlsym(self.0, c.as_ptr());
        if p.is_null() {
            return Err(format!("no {name} in the library"));
        }
        Ok(std::mem::transmute_copy::<*mut c_void, F>(&p))
    }
}
