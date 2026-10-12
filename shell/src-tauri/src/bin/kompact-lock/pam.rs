//! 14a: check the user's password with PAM (the way swaylock does): `pam_start` with our
//! conversation (it answers every hidden prompt with the password), `pam_authenticate`,
//! `pam_acct_mgmt`, then `pam_setcred(PAM_REFRESH_CRED)` so a Kerberos ticket or keyring gets
//! the news, and `pam_end`. The password lives in one buffer, zeroed when done; PAM frees its
//! own copy.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

use crate::dl::Lib;

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_REFRESH_CRED: c_int = 0x0010;

#[repr(C)]
struct Message {
    style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct Response {
    resp: *mut c_char,
    retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int;

#[repr(C)]
struct Conv {
    conv: ConvFn,
    appdata: *mut c_void,
}

struct Api {
    start: unsafe extern "C" fn(*const c_char, *const c_char, *const Conv, *mut *mut c_void) -> c_int,
    authenticate: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    acct_mgmt: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    setcred: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    end: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    strerror: unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let lib = Lib::open("libpam.so.0")?;
        // SAFETY: the real libpam signatures (security/pam_appl.h).
        unsafe {
            Ok(Api {
                start: lib.get("pam_start")?,
                authenticate: lib.get("pam_authenticate")?,
                acct_mgmt: lib.get("pam_acct_mgmt")?,
                setcred: lib.get("pam_setcred")?,
                end: lib.get("pam_end")?,
                strerror: lib.get("pam_strerror")?,
            })
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Answers each password prompt with `appdata` (a NUL-terminated password).
unsafe extern "C" fn converse(n: c_int, msgs: *mut *const Message, out: *mut *mut Response, appdata: *mut c_void) -> c_int {
    if n <= 0 || n > 32 || msgs.is_null() || out.is_null() {
        return PAM_CONV_ERR;
    }
    let replies = libc::calloc(n as usize, std::mem::size_of::<Response>()) as *mut Response;
    if replies.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..n as usize {
        // Linux-PAM: an array of `n` pointers to messages.
        let m = *msgs.add(i);
        if m.is_null() {
            continue;
        }
        let r = &mut *replies.add(i);
        match (*m).style {
            PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                r.resp = libc::strdup(appdata as *const c_char);
                if r.resp.is_null() {
                    for j in 0..i {
                        let p = (*replies.add(j)).resp;
                        if !p.is_null() {
                            libc::free(p as *mut c_void);
                        }
                    }
                    libc::free(replies as *mut c_void);
                    return PAM_BUF_ERR;
                }
            }
            // Info and error text: nothing to answer.
            _ => {}
        }
    }
    *out = replies;
    PAM_SUCCESS
}

/// Whether `password` is `user`'s, through PAM `service`. Blocks (PAM waits ~3 s after a wrong
/// one): run it off the GTK thread. `Err` carries PAM's own reason.
pub fn check(service: &str, user: &str, mut password: Vec<u8>) -> Result<(), String> {
    let api = api()?;
    password.retain(|b| *b != 0);
    password.push(0);
    let result = (|| {
        let service = CString::new(service).map_err(|e| e.to_string())?;
        let user = CString::new(user).map_err(|e| e.to_string())?;
        let conv = Conv { conv: converse, appdata: password.as_mut_ptr() as *mut c_void };
        let mut h: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid C strings and conversation; `h` is ended below on every path.
        unsafe {
            let rc = (api.start)(service.as_ptr(), user.as_ptr(), &conv, &mut h);
            if rc != PAM_SUCCESS || h.is_null() {
                return Err(format!("PAM won't start ({rc})"));
            }
            let mut rc = (api.authenticate)(h, 0);
            if rc == PAM_SUCCESS {
                rc = (api.acct_mgmt)(h, 0);
            }
            if rc == PAM_SUCCESS {
                // Not fatal: the password was right.
                let _ = (api.setcred)(h, PAM_REFRESH_CRED);
            }
            let why = if rc == PAM_SUCCESS {
                None
            } else {
                let s = (api.strerror)(h, rc);
                Some(if s.is_null() { format!("PAM error {rc}") } else { CStr::from_ptr(s).to_string_lossy().into_owned() })
            };
            (api.end)(h, rc);
            why.map_or(Ok(()), Err)
        }
    })();
    for b in password.iter_mut() {
        // SAFETY: a plain byte write the compiler mustn't drop.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    result
}
