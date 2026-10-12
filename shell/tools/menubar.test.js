// 13d: checks web/menubar.js's rules. Run: gjs shell/tools/menubar.test.js (prints "ok").
const GLib = imports.gi.GLib;
const dir = GLib.path_get_dirname(imports.system.programInvocationName);
const src = new TextDecoder().decode(GLib.file_get_contents(`${dir}/../web/menubar.js`)[1]);
const m = new Function(`${src}; return { menuBarShows, menuBarFolds };`)();
let failed = 0;
function eq(got, want, what) {
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    failed++;
    print(`FAIL ${what}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
  }
}
const shows = m.menuBarShows;
// Network: hidden on a working wired connection without Wi-Fi.
eq(shows("network", "active", { network: { icon: "ethernet", has_wifi: false } }), false, "wired");
eq(shows("network", "active", { network: { icon: "ethernet", has_wifi: true } }), true, "wired + wifi card");
eq(shows("network", "active", { network: { icon: "none", has_wifi: false } }), true, "offline shows");
eq(shows("network", "always", { network: { icon: "ethernet", has_wifi: false } }), true, "always");
eq(shows("network", "never", { network: { icon: "wifi", has_wifi: true } }), false, "never");
eq(shows("network", undefined, {}), false, "not known yet");
// Bluetooth: a connected device.
const bt = (powered, connected, adapter = "hci0") => ({ bluetooth: { adapter, powered, connected } });
eq(shows("bluetooth", "active", bt(true, 0)), false, "bt idle");
eq(shows("bluetooth", "active", bt(true, 1)), true, "bt connected");
eq(shows("bluetooth", "active", bt(false, 1)), false, "bt off");
eq(shows("bluetooth", "always", bt(false, 0)), true, "bt always");
eq(shows("bluetooth", "always", bt(true, 1, null)), false, "no adapter");
// Sound: muted or playing.
eq(shows("sound", "active", { sound: { output: true, muted: false, playing: [] } }), false, "silent");
eq(shows("sound", "active", { sound: { output: true, muted: false, playing: ["Firefox"] } }), true, "playing");
eq(shows("sound", "active", { sound: { output: true, muted: true } }), true, "muted");
eq(shows("sound", "always", { sound: { output: false } }), false, "no output");
// Input: two layouts or more.
eq(shows("input", "active", { input: { layouts: 1 } }), false, "one layout");
eq(shows("input", "active", { input: { layouts: 2 } }), true, "two layouts");
eq(shows("input", "always", { input: { layouts: 1 } }), true, "always one");
// Desktops: away from desktop 1 or windows elsewhere.
eq(shows("spaces", "active", { spaces: { current: 1, others_in_use: false } }), false, "desktop 1 only");
eq(shows("spaces", "active", { spaces: { current: 3, others_in_use: false } }), true, "on desktop 3");
eq(shows("spaces", "active", { spaces: { current: 1, others_in_use: true } }), true, "windows elsewhere");
eq(shows("spotlight", undefined, {}), true, "spotlight default");
eq(shows("spotlight", "never", {}), false, "spotlight never");
// Folding, least needed first, until enough is freed.
eq(m.menuBarFolds({ network: 30, sound: 30, spotlight: 30, spaces: 30 }, 0), [], "fits");
eq(m.menuBarFolds({ network: 30, sound: 30, spotlight: 30, spaces: 30 }, 40), ["spaces", "spotlight"], "two");
eq(m.menuBarFolds({ network: 30, sound: 30 }, 1000), ["sound", "network"], "all it can");
print(failed ? `${failed} failed` : "ok");
