//! 12i-2: Sleep, and locking the screen before the computer sleeps.
//!
//! The logo menu's Sleep runs `systemctl suspend`, but only with `KOMPACT_SLEEP=on`: kompact's
//! suspend/resume on the GT 710 (nouveau) is untested, and a bad KMS state after resume may need
//! a hard reboot. Locking first is always on: a logind *delay* inhibitor holds the sleep (from
//! anywhere: the menu, `systemctl suspend`, a power key) until kompact says the screen is locked
//! (its `lock` message), or 2 s at most; after waking it is taken again.

use std::time::Duration;

use tauri::{AppHandle, Manager};
use tracing::{info, warn};

/// Longest wait for the lock screen before letting the computer sleep anyway (logind's own
/// limit, `InhibitDelayMaxSec`, is 30 s here).
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// `KOMPACT_SLEEP=on` shows Sleep in the logo menu.
pub fn sleep_enabled() -> bool {
    std::env::var("KOMPACT_SLEEP").is_ok_and(|v| v.trim().eq_ignore_ascii_case("on"))
}

/// Whether kompact's screen is locked, from its `lock` messages.
pub struct LockWatch(tokio::sync::watch::Sender<bool>);

impl Default for LockWatch {
    fn default() -> Self {
        LockWatch(tokio::sync::watch::channel(false).0)
    }
}

impl LockWatch {
    pub fn set(&self, locked: bool) {
        self.0.send_replace(locked);
    }

    /// 12i-1b: for org.freedesktop.ScreenSaver's `GetActive`.
    pub fn locked(&self) -> bool {
        *self.0.borrow()
    }
}

/// What the logo menu offers.
#[tauri::command]
pub fn power_options() -> serde_json::Value {
    serde_json::json!({ "sleep": sleep_enabled() })
}

async fn login1(conn: &zbus::Connection) -> zbus::Result<zbus::Proxy<'static>> {
    zbus::proxy::Builder::<zbus::Proxy>::new(conn)
        .destination("org.freedesktop.login1")?
        .path("/org/freedesktop/login1")?
        .interface("org.freedesktop.login1.Manager")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

/// A delay lock on sleep; held until it's dropped.
async fn inhibit(proxy: &zbus::Proxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
    match proxy.call("Inhibit", &("sleep", "kompact", "Locking the screen first", "delay")).await {
        Ok(fd) => Some(fd),
        Err(e) => {
            warn!("sleep: can't hold sleep to lock the screen first: {e}");
            None
        }
    }
}

/// Watch logind's `PrepareForSleep`: lock, then let it sleep.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let conn = match zbus::Connection::system().await {
            Ok(c) => c,
            Err(e) => return warn!("sleep: system bus: {e}; the screen won't lock before sleeping"),
        };
        let proxy = match login1(&conn).await {
            Ok(p) => p,
            Err(e) => return warn!("sleep: logind: {e}; the screen won't lock before sleeping"),
        };
        let mut signals = match proxy.receive_signal("PrepareForSleep").await {
            Ok(s) => s,
            Err(e) => return warn!("sleep: can't watch logind: {e}; the screen won't lock before sleeping"),
        };
        let mut held = inhibit(&proxy).await;
        info!("sleep: the screen locks before the computer sleeps{}", if sleep_enabled() { "; Sleep is in the logo menu (KOMPACT_SLEEP=on)" } else { "" });
        while let Some(msg) = signals.next().await {
            let Ok(going) = msg.body().deserialize::<bool>() else { continue };
            if going {
                info!("sleep: the computer is going to sleep, locking the screen first");
                if let Err(e) = app.state::<crate::compositor::Compositor>().send_line_pub("lock", "lock\n") {
                    warn!("sleep: can't ask kompact to lock: {e}");
                }
                let mut locked = app.state::<LockWatch>().0.subscribe();
                match tokio::time::timeout(LOCK_WAIT, locked.wait_for(|l| *l)).await {
                    Ok(Ok(_)) => info!("sleep: screen locked, sleeping"),
                    _ => warn!("sleep: the screen didn't lock within {} s, sleeping anyway", LOCK_WAIT.as_secs()),
                }
                drop(held.take());
            } else {
                info!("sleep: woke up");
                if held.is_none() {
                    held = inhibit(&proxy).await;
                }
            }
        }
        warn!("sleep: logind's signals stopped");
    });
}
