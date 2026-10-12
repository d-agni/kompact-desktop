//! 6p-f: offer linux-dmabuf, so apps can draw on the GPU (on by default since 6p-g;
//! `KOMPACT_DMABUF=off` goes back to shared memory only).
//!
//! Without it every app draws into shared memory and kompact copies each changed area to a GL
//! texture; 6p-e left Firefox sending a whole-window frame (≈1.3 Mpx) each commit. With the
//! `zwp_linux_dmabuf_v1` global (version 4, default feedback naming the GPU kompact renders on),
//! Mesa's EGL in the apps can hand over GPU buffers, which kompact samples without a copy.
//!
//! Client buffers only ever become GL textures: the DRM backend composes every frame with
//! `FrameFlags::empty()`, so none reaches a KMS plane, and the KMS commits don't change. Each
//! buffer is imported into the renderer when the client creates it (`dmabuf_imported`), so a
//! buffer nouveau can't take is refused to that client then, not found broken while rendering.

/// On unless `off`/`0`/`no`/`false` (6p-g; confirmed on the GT 710: missed frames 328 → 54/min).
pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

pub fn enabled() -> bool {
    enabled_from_env(std::env::var("KOMPACT_DMABUF").ok().as_deref())
}

/// Log the first few refused imports, then every 100th, so a client retrying can't flood the log.
pub fn log_failure(count: u64) -> bool {
    count <= 5 || count % 100 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_unless_switched_off() {
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("")));
        assert!(enabled_from_env(Some("on")));
        assert!(!enabled_from_env(Some("off")));
        assert!(!enabled_from_env(Some(" No ")));
    }

    #[test]
    fn failures_logged_sparingly() {
        let logged: Vec<u64> = (1..=300).filter(|&n| log_failure(n)).collect();
        assert_eq!(logged, [1, 2, 3, 4, 5, 100, 200, 300]);
    }
}
