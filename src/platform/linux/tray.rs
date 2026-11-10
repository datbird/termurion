//! The resident icon on Linux: a StatusNotifierItem on the session bus, the icon GNOME's
//! AppIndicator extension, KDE and most panels show. The panel draws the menu itself from a
//! `com.canonical.dbusmenu` object, so the app's tray menu is exported, not popped up: the UI
//! thread hands its muda tree over (`export`) and the bus thread serves it. A left click is
//! `Activate`; a menu pick comes back by its muda id, as a popup pick does.
//!
//! One connection per icon: dropping it takes the icon down (the watcher follows the bus name).
//! A second connection, started with the app, follows whether a watcher exists at all, so a
//! watcher that restarts gets the icon again (Windows' TaskbarCreated).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, StructureBuilder, Value};

use crate::dlog;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
/// How long a call to the watcher may take.
const CALL_BOUND: Duration = Duration::from_secs(2);
/// How long the panel's AboutToShow waits for the app to rebuild the menu.
const REBUILD_WAIT: Duration = Duration::from_millis(250);

/// The "view" the tray menu is anchored on while the icon is up: no window has it, so a popup on
/// it is exported to the panel instead of drawn.
pub const VIEW: isize = -0x5452_4159;

type Hook<T> = Mutex<Option<Arc<T>>>;

static CLICK: Hook<dyn Fn(bool) + Send + Sync> = Mutex::new(None);
static PICK: Hook<dyn Fn(String) + Send + Sync> = Mutex::new(None);
static HOST_BACK: Hook<dyn Fn() + Send + Sync> = Mutex::new(None);
/// What the app hears when the icon cannot bring it back any more: an add failed (with why), or
/// the watcher left (None).
static LOST: Hook<dyn Fn(Option<String>) + Send + Sync> = Mutex::new(None);
/// The icon's own connection while it is up.
static ITEM: Mutex<Option<Connection>> = Mutex::new(None);
/// Whether a watcher owns its name now (followed by `watch`).
static HOST_UP: AtomicBool = AtomicBool::new(false);
static ITEM_SEQ: AtomicU32 = AtomicU32::new(0);
/// Adds run on threads of their own (the UI never waits on the bus); this counts those running.
static ADDING: AtomicU32 = AtomicU32::new(0);
/// The last add failed (cleared by a success or a watcher coming back): not up (review N4).
static ADD_FAILED: AtomicBool = AtomicBool::new(false);
/// The watch thread has had its first answer (the watcher there or not).
static WATCH_KNOWN: AtomicBool = AtomicBool::new(false);
/// When the watch started: its first answer is waited for at most `KNOWN_WAIT` (review N3).
static WATCH_STARTED: OnceLock<std::time::Instant> = OnceLock::new();
/// How long a start waits for the watcher's first answer before it counts as absent.
pub const KNOWN_WAIT: Duration = Duration::from_secs(5);
/// Bumped by every `set`: an add that finishes after a newer `set` keeps nothing.
static WANT_GEN: AtomicU64 = AtomicU64::new(0);
/// The activation token the panel gave before its last Activate (KDE's ProvideXdgActivationToken).
static TOKEN: Mutex<Option<String>> = Mutex::new(None);
/// The exported menu and its revision; the condvar wakes an AboutToShow waiting for a rebuild.
static MENU: Mutex<Exported> = Mutex::new(Exported { table: BTreeMap::new(), revision: 0, ids: Ids { by_key: None, next: 0 } });
static MENU_CHANGED: Condvar = Condvar::new();

fn lock<T: ?Sized>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn run<T: ?Sized>(hook: &Hook<T>) -> Option<Arc<T>> {
    lock(hook).clone()
}

pub fn on_click(f: Box<dyn Fn(bool) + Send + Sync>) {
    *lock(&CLICK) = Some(Arc::from(f));
}

pub fn on_pick(f: Box<dyn Fn(String) + Send + Sync>) {
    *lock(&PICK) = Some(Arc::from(f));
}

/// What a watcher coming (back) onto the bus does: the app adds the icon again.
pub fn on_host_back(f: Box<dyn Fn() + Send + Sync>) {
    *lock(&HOST_BACK) = Some(Arc::from(f));
}

/// What the app hears when the icon stops being a way back (see `LOST`).
pub fn on_lost(f: Box<dyn Fn(Option<String>) + Send + Sync>) {
    *lock(&LOST) = Some(Arc::from(f));
}

fn tell_lost(why: Option<String>) {
    if let Some(f) = run(&LOST) {
        f(why);
    }
}

/// Whether a panel could show the icon right now (a watcher is on the bus).
pub fn host_up() -> bool {
    HOST_UP.load(Ordering::SeqCst)
}

/// Where the icon stands: up only while the watcher that took it is still on the bus.
pub fn state() -> crate::resident::Icon {
    // Before the watch started (tests, or no app) nothing is waited for.
    let waiting = !WATCH_KNOWN.load(Ordering::SeqCst) && WATCH_STARTED.get().is_some_and(|t| t.elapsed() < KNOWN_WAIT);
    icon_of(waiting, host_up(), lock(&ITEM).is_some(), ADDING.load(Ordering::SeqCst) > 0, ADD_FAILED.load(Ordering::SeqCst))
}

/// `waiting`: the watcher's first answer has not come and the bounded wait is not over.
pub fn icon_of(waiting: bool, host: bool, item: bool, adding: bool, failed: bool) -> crate::resident::Icon {
    use crate::resident::Icon;
    match (host, item, adding, failed) {
        (false, ..) if waiting => Icon::Unknown,
        (false, ..) => Icon::Absent,
        (true, true, ..) => Icon::Up,
        (true, false, true, _) => Icon::Adding,
        (true, false, false, true) => Icon::Retrying,
        (true, false, false, false) => Icon::Idle,
    }
}

/// The token the panel handed over for the click that is being handled, once.
pub fn take_token() -> Option<String> {
    lock(&TOKEN).take()
}

/// The menu anchor while the icon is up.
pub fn view() -> Option<isize> {
    lock(&ITEM).is_some().then_some(VIEW)
}

// ---- The menu, as plain data ----

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Item,
    Check(bool),
    Submenu,
    Separator,
}

/// One row of the menu, read from a muda item on the UI thread (muda items are not Send).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub enabled: bool,
    pub kind: Kind,
    /// The muda id a click sends back.
    pub pick: String,
    pub children: Vec<Row>,
}

/// A muda label as dbusmenu shows it: Windows' `&&` is one `&`, a single `&` mnemonic becomes
/// dbusmenu's `_`, a real `_` is doubled, and the shortcut after the tab is dropped (the panel
/// draws none).
pub fn label_of(text: &str) -> String {
    let text = text.split('\t').next().unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '_' => out.push_str("__"),
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                out.push('&');
            }
            '&' => {
                // A trailing lone `&` marks nothing.
                if chars.peek().is_some() {
                    out.push('_');
                }
            }
            c => out.push(c),
        }
    }
    out
}

pub fn rows_of(items: &[muda::MenuItemKind]) -> Vec<Row> {
    use muda::MenuItemKind as K;
    items
        .iter()
        .map(|k| match k {
            K::MenuItem(i) => Row { label: label_of(&i.text()), enabled: i.is_enabled(), kind: Kind::Item, pick: i.id().0.clone(), children: Vec::new() },
            K::Check(i) => Row { label: label_of(&i.text()), enabled: i.is_enabled(), kind: Kind::Check(i.is_checked()), pick: i.id().0.clone(), children: Vec::new() },
            K::Icon(i) => Row { label: label_of(&i.text()), enabled: i.is_enabled(), kind: Kind::Item, pick: i.id().0.clone(), children: Vec::new() },
            K::Submenu(s) => Row { label: label_of(&s.text()), enabled: s.is_enabled(), kind: Kind::Submenu, pick: s.id().0.clone(), children: rows_of(&s.items()) },
            // A separator has no text; any other predefined item is a plain row (as the in-window menu draws it).
            K::Predefined(p) => {
                let text = p.text();
                let kind = if text.is_empty() { Kind::Separator } else { Kind::Item };
                Row { label: label_of(&text), enabled: true, kind, pick: p.id().0.clone(), children: Vec::new() }
            }
        })
        .collect()
}

/// A row with its dbusmenu id (the table's key; 0 is the root).
#[derive(Clone, Debug, PartialEq)]
pub struct Flat {
    pub row: Option<Row>,
    pub children: Vec<i32>,
}

/// The dbusmenu ids, kept across exports: a row keeps its id while it is in the menu, and an id is
/// never given to another row, so a click on a menu the panel showed before a rebuild runs the
/// same command or nothing (review I4).
#[derive(Debug)]
pub struct Ids {
    /// Row key to id, for the rows of the last export.
    by_key: Option<HashMap<String, i32>>,
    next: i32,
}

impl Default for Ids {
    fn default() -> Self {
        Ids { by_key: None, next: 0 }
    }
}

/// Numbers the rows under the root 0: a command row by its muda id, any other row by its place
/// (parent and label). New rows take ids after every id given before.
pub fn flatten(rows: &[Row], ids: &mut Ids) -> BTreeMap<i32, Flat> {
    struct Walk<'a> {
        table: BTreeMap<i32, Flat>,
        old: HashMap<String, i32>,
        fresh: HashMap<String, i32>,
        ids: &'a mut Ids,
    }
    fn add(w: &mut Walk<'_>, rows: &[Row], parent: &str) -> Vec<i32> {
        let mut out = Vec::new();
        for r in rows {
            let base = match r.kind {
                Kind::Item | Kind::Check(_) => format!("p:{}", r.pick),
                Kind::Submenu | Kind::Separator => format!("{parent}/{}", r.label),
            };
            // The same command twice (New Window at the top and in Main Menu) gets two ids.
            let mut key = base.clone();
            let mut n = 1;
            while w.fresh.contains_key(&key) {
                key = format!("{base}#{n}");
                n += 1;
            }
            let id = match w.old.get(&key) {
                Some(id) => *id,
                None => {
                    w.ids.next = w.ids.next.wrapping_add(1).max(1);
                    w.ids.next
                }
            };
            w.fresh.insert(key.clone(), id);
            w.table.insert(id, Flat { row: Some(Row { children: Vec::new(), ..r.clone() }), children: Vec::new() });
            let kids = add(w, &r.children, &key);
            if let Some(f) = w.table.get_mut(&id) {
                f.children = kids;
            }
            out.push(id);
        }
        out
    }
    let old = ids.by_key.take().unwrap_or_default();
    let mut w = Walk { table: BTreeMap::new(), old, fresh: HashMap::new(), ids };
    let kids = add(&mut w, rows, "");
    let Walk { mut table, fresh, ids, .. } = w;
    table.insert(0, Flat { row: None, children: kids });
    ids.by_key = Some(fresh);
    table
}

fn owned(v: Value<'_>) -> OwnedValue {
    // Only a file descriptor can fail, and none is ever put in.
    v.try_into_owned().unwrap_or_else(|_| OwnedValue::from(0u8))
}

/// The dbusmenu properties of one entry, only those in `names` when it names any.
pub fn props(f: &Flat, names: &[String]) -> HashMap<String, OwnedValue> {
    let mut p: Vec<(&str, OwnedValue)> = Vec::new();
    match &f.row {
        None => p.push(("children-display", owned(Value::from("submenu")))),
        Some(r) if r.kind == Kind::Separator => p.push(("type", owned(Value::from("separator")))),
        Some(r) => {
            p.push(("label", owned(Value::from(r.label.as_str()))));
            p.push(("enabled", owned(Value::from(r.enabled))));
            match r.kind {
                Kind::Check(on) => {
                    p.push(("toggle-type", owned(Value::from("checkmark"))));
                    p.push(("toggle-state", owned(Value::from(i32::from(on)))));
                }
                Kind::Submenu => p.push(("children-display", owned(Value::from("submenu")))),
                Kind::Item | Kind::Separator => {}
            }
        }
    }
    p.into_iter().filter(|(k, _)| names.is_empty() || names.iter().any(|n| n.as_str() == *k)).map(|(k, v)| (k.to_string(), v)).collect()
}

/// The `(ia{sv}av)` of `id` and its children down `depth` levels (-1: all).
fn node(table: &BTreeMap<i32, Flat>, id: i32, depth: i32, names: &[String]) -> (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>) {
    let Some(f) = table.get(&id) else { return (id, HashMap::new(), Vec::new()) };
    let kids = if depth == 0 {
        Vec::new()
    } else {
        f.children
            .iter()
            .map(|c| {
                let (cid, cp, ck) = node(table, *c, depth - 1, names);
                let s = StructureBuilder::new().add_field(cid).add_field(cp).add_field(ck).build();
                s.map(|s| owned(Value::from(s))).unwrap_or_else(|_| OwnedValue::from(0u8))
            })
            .collect()
    };
    (id, props(f, names), kids)
}

struct Exported {
    table: BTreeMap<i32, Flat>,
    revision: u32,
    ids: Ids,
}

/// The UI thread's tray menu, as the panel will show it next. Tells the panel it changed.
pub fn export(items: &[muda::MenuItemKind]) {
    export_rows(&rows_of(items));
}

fn export_rows(rows: &[Row]) {
    let revision = {
        let mut m = lock(&MENU);
        let m = &mut *m;
        m.table = flatten(rows, &mut m.ids);
        m.revision = m.revision.wrapping_add(1);
        m.revision
    };
    MENU_CHANGED.notify_all();
    // The signal goes out on a thread of its own: the UI never waits on the bus. The thread takes
    // the connection only if it is still the icon's, so a removal is not held up by it (review M6).
    let Some(name) = lock(&ITEM).as_ref().and_then(|c| c.unique_name().map(|n| n.to_string())) else { return };
    let _ = std::thread::Builder::new().name("tray-layout".into()).spawn(move || {
        let Some(c) = lock(&ITEM).clone().filter(|c| c.unique_name().is_some_and(|n| n.as_str() == name)) else { return };
        let _ = c.emit_signal(None::<()>, MENU_PATH, "com.canonical.dbusmenu", "LayoutUpdated", &(revision, 0i32));
    });
}

// ---- The icon ----

/// ARGB32 in network byte order, as StatusNotifierItem pixmaps are, from RGBA.
pub fn argb_of(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect()
}

fn pixmaps() -> Vec<(i32, i32, Vec<u8>)> {
    static PIX: OnceLock<Vec<(i32, i32, Vec<u8>)>> = OnceLock::new();
    PIX.get_or_init(|| {
        let mut out: Vec<(i32, i32, Vec<u8>)> = [16, 32, 48].into_iter().map(crate::presence::icon_rgba).filter(|(_, s)| *s > 0).map(|(rgba, s)| (s as i32, s as i32, argb_of(&rgba))).collect();
        // The AppImage's 256 px art, for a panel on a large scale.
        if let Ok(img) = image::load_from_memory_with_format(include_bytes!("../../../docs/assets/termurion/termurion-icon-256.png"), image::ImageFormat::Png) {
            let rgba = img.into_rgba8();
            out.push((rgba.width() as i32, rgba.height() as i32, argb_of(rgba.as_raw())));
        }
        out
    })
    .clone()
}

struct Item;

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "termurion"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "Termurion"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        pixmaps()
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<(i32, i32, Vec<u8>)>, String, String) {
        (String::new(), Vec::new(), "Termurion".into(), String::new())
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("a valid path")
    }

    /// KDE's extension: the token for the click that follows, so the window may come front.
    fn provide_xdg_activation_token(&self, token: String) {
        *lock(&TOKEN) = crate::launch::token_ok(&token).then_some(token);
    }

    /// A left click: the dock click, as on Windows and macOS.
    fn activate(&self, _x: i32, _y: i32) {
        dlog!("tray: activate");
        if let Some(f) = run(&CLICK) {
            f(false);
        }
    }
    /// A host without menus asks for one here; the menu is the exported one, so nothing to do.
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn secondary_activate(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
}

struct DbusMenu;

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl DbusMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    fn get_layout(&self, parent: i32, depth: i32, names: Vec<String>) -> (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)) {
        let m = lock(&MENU);
        (m.revision, node(&m.table, parent, depth, &names))
    }

    fn get_group_properties(&self, ids: Vec<i32>, names: Vec<String>) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let m = lock(&MENU);
        let all: Vec<i32> = if ids.is_empty() { m.table.keys().copied().collect() } else { ids };
        all.into_iter().filter_map(|id| m.table.get(&id).map(|f| (id, props(f, &names)))).collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let m = lock(&MENU);
        let f = m.table.get(&id).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no item {id}")))?;
        props(f, &[name.clone()]).remove(&name).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name}")))
    }

    fn event(&self, id: i32, event: String, _data: OwnedValue, _time: u32) {
        clicked(id, &event);
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event, _, _) in &events {
            clicked(*id, event);
        }
        Vec::new()
    }

    /// The panel opens the menu: the app builds it again (the window list may have changed), and
    /// the panel waits a moment for it.
    fn about_to_show(&self, id: i32) -> bool {
        id == 0 && rebuild()
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let updated = if ids.contains(&0) && rebuild() { vec![0] } else { Vec::new() };
        (updated, Vec::new())
    }
}

/// A click on a menu row: its muda id goes back to the app. A submenu's own row picks nothing.
fn clicked(id: i32, event: &str) {
    if event != "clicked" {
        return;
    }
    // A token given for an Activate that never came is not left for a later one (review N5).
    lock(&TOKEN).take();
    let pick = lock(&MENU).table.get(&id).and_then(|f| f.row.as_ref()).filter(|r| matches!(r.kind, Kind::Item | Kind::Check(_)) && r.enabled).map(|r| r.pick.clone());
    dlog!("tray: menu row {} clicked, picks {}", id, pick.is_some());
    if let (Some(p), Some(f)) = (pick, run(&PICK)) {
        f(p);
    }
}

/// Asks the app for a fresh menu and waits a moment for it. True when it came.
fn rebuild() -> bool {
    let Some(f) = run(&CLICK) else { return false };
    let before = lock(&MENU).revision;
    f(true);
    let m = lock(&MENU);
    let (m, _) = MENU_CHANGED.wait_timeout_while(m, REBUILD_WAIT, |m| m.revision == before).unwrap_or_else(|e| e.into_inner());
    m.revision != before
}

fn no_host_text() -> String {
    "No panel shows tray icons here: nothing owns org.kde.StatusNotifierWatcher on the session bus (on GNOME, turn on the AppIndicator extension)".to_string()
}

/// Puts the icon up on a connection from `bus`: its objects, its name, and the watcher told.
fn add_on(bus: zbus::blocking::connection::Builder<'_>) -> Result<Connection, String> {
    let name = format!("org.kde.StatusNotifierItem-{}-{}", std::process::id(), ITEM_SEQ.fetch_add(1, Ordering::SeqCst) + 1);
    let c = bus
        .method_timeout(CALL_BOUND)
        .serve_at(ITEM_PATH, Item)
        .and_then(|b| b.serve_at(MENU_PATH, DbusMenu))
        .and_then(|b| b.name(name.clone()))
        .and_then(|b| b.build())
        .map_err(|e| format!("Termurion could not reach the session bus for its tray icon: {e}"))?;
    match c.call_method(Some(WATCHER), WATCHER_PATH, Some(WATCHER), "RegisterStatusNotifierItem", &(name.as_str(),)) {
        Ok(_) => Ok(c),
        Err(zbus::Error::MethodError(n, _, _)) if n.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || n.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => Err(no_host_text()),
        Err(e) => Err(format!("The tray icon was not added: {e}")),
    }
}

/// The add on this thread, as the bus tests drive it.
#[cfg(test)]
fn add_with(bus: impl FnOnce() -> zbus::Result<zbus::blocking::connection::Builder<'static>>) -> Result<(), String> {
    add_for(bus, WANT_GEN.load(Ordering::SeqCst))
}

/// The add for the `set` numbered `generation`: a newer `set` meanwhile keeps nothing of it.
fn add_for(bus: impl FnOnce() -> zbus::Result<zbus::blocking::connection::Builder<'static>>, generation: u64) -> Result<(), String> {
    let b = bus().map_err(|e| format!("Termurion could not reach the session bus for its tray icon: {e}"))?;
    let c = add_on(b)?;
    {
        let mut item = lock(&ITEM);
        if WANT_GEN.load(Ordering::SeqCst) != generation {
            return Ok(());
        }
        // An icon that was up already is replaced (a watcher that came back).
        *item = Some(c);
    }
    ADD_FAILED.store(false, Ordering::SeqCst);
    crate::host_base::log("tray: icon registered with the StatusNotifierWatcher");
    // The panel reads the menu before any click: the app builds the first one now.
    if let Some(f) = run(&CLICK) {
        f(true);
    }
    Ok(())
}

/// Adds the icon, or takes it down. The add runs on a thread of its own (review M5): Ok means it
/// started, and a failure reaches the app through `on_lost`. Taking it down drops the connection,
/// which the watcher sees.
pub fn set(on: bool) -> Result<(), String> {
    set_with(on, zbus::blocking::connection::Builder::session)
}

fn set_with(on: bool, bus: impl FnOnce() -> zbus::Result<zbus::blocking::connection::Builder<'static>> + Send + 'static) -> Result<(), String> {
    let generation = WANT_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if !on {
        lock(&ITEM).take();
        return Ok(());
    }
    if !host_up() {
        // Before the watcher's first answer the add is only early: the retry comes in a moment.
        return Err(if state() == crate::resident::Icon::Unknown { "The tray icon waits for the panel to answer".to_string() } else { no_host_text() });
    }
    ADDING.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("tray-add".into()).spawn(move || {
        let result = add_for(bus, generation);
        ADDING.fetch_sub(1, Ordering::SeqCst);
        if let Err(e) = result
            && WANT_GEN.load(Ordering::SeqCst) == generation
        {
            ADD_FAILED.store(true, Ordering::SeqCst);
            tell_lost(Some(e));
        }
    });
    if let Err(e) = spawned {
        ADDING.fetch_sub(1, Ordering::SeqCst);
        return Err(format!("The tray icon was not added: {e}"));
    }
    Ok(())
}

// ---- Following the watcher ----

/// A NameOwnerChanged for the watcher's name, from the bus itself: the new owner ("" when gone).
fn watcher_change(msg: &zbus::Message) -> Option<String> {
    let header = msg.header();
    if header.sender().map(|s| s.as_str()) != Some("org.freedesktop.DBus") || header.member().map(|m| m.as_str()) != Some("NameOwnerChanged") {
        return None;
    }
    let (name, _, new) = msg.body().deserialize::<(String, String, String)>().ok()?;
    (name == WATCHER).then_some(new)
}

/// Starts following the watcher on a connection from `bus`, once per process.
fn watch_on(bus: impl FnOnce() -> zbus::Result<zbus::blocking::connection::Builder<'static>> + Send + 'static) {
    let spawned = std::thread::Builder::new().name("tray-watch".into()).spawn(move || {
        // Whatever happens below, the wait for a first answer ends with this thread.
        struct Known;
        impl Drop for Known {
            fn drop(&mut self) {
                WATCH_KNOWN.store(true, Ordering::SeqCst);
            }
        }
        let _known = Known;
        let Ok(c) = bus().and_then(|b| b.method_timeout(CALL_BOUND).build()) else {
            dlog!("tray: no session bus, so no tray icon");
            return;
        };
        let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&c) else { return };
        // The match goes first, so a move between it and the ask is not missed.
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender("org.freedesktop.DBus").and_then(|b| b.interface("org.freedesktop.DBus")).and_then(|b| b.member("NameOwnerChanged")).and_then(|b| b.add_arg(WATCHER)).map(|b| b.build());
        let Ok(rule) = rule else { return };
        if proxy.add_match_rule(rule).is_err() {
            return;
        }
        let messages = zbus::blocking::MessageIterator::from(c);
        let up = zbus::names::BusName::try_from(WATCHER).ok().and_then(|n| proxy.name_has_owner(n).ok()).unwrap_or(false);
        HOST_UP.store(up, Ordering::SeqCst);
        WATCH_KNOWN.store(true, Ordering::SeqCst);
        dlog!("tray: watcher present {}", up);
        let mut why = "the session bus closed".to_string();
        for msg in messages {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    why = e.to_string();
                    break;
                }
            };
            let Some(new) = watcher_change(&msg) else { continue };
            let up = !new.is_empty();
            HOST_UP.store(up, Ordering::SeqCst);
            crate::host_base::log(&format!("tray: StatusNotifierWatcher {}", if up { "came onto the bus" } else { "left the bus" }));
            if up {
                // A new watcher gets a fresh try.
                ADD_FAILED.store(false, Ordering::SeqCst);
                if let Some(f) = run(&HOST_BACK) {
                    f();
                }
            } else {
                tell_lost(None);
            }
        }
        // No longer followed: from now on there is no panel to count on (review I1, scenario C).
        HOST_UP.store(false, Ordering::SeqCst);
        crate::host_base::log(&format!("tray: stopped following the StatusNotifierWatcher ({why})"));
        tell_lost(None);
    });
    if spawned.is_err() {
        dlog!("tray: the watcher thread did not start");
    }
}

/// Follows the session bus's watcher from now on (the app calls it once, at its start).
pub fn watch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = WATCH_STARTED.set(std::time::Instant::now());
        watch_on(zbus::blocking::connection::Builder::session)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    /// The menu, the hooks and the icon are process wide, so the bus tests take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn row(label: &str, kind: Kind, pick: &str, children: Vec<Row>) -> Row {
        Row { label: label.into(), enabled: true, kind, pick: pick.into(), children }
    }

    #[test]
    fn pixels_become_argb_in_network_order() {
        assert_eq!(argb_of(&[1, 2, 3, 4, 5, 6, 7, 8]), vec![4, 1, 2, 3, 8, 5, 6, 7]);
        assert!(argb_of(&[1, 2, 3]).is_empty(), "a partial pixel is dropped");
        let pix = pixmaps();
        assert!(pix.iter().any(|(w, h, _)| *w == 16 && *h == 16) && pix.iter().any(|(w, _, _)| *w == 256), "the small art and the AppImage's 256 px");
        assert!(pix.iter().all(|(w, h, d)| d.len() == (*w * *h * 4) as usize));
    }

    #[test]
    fn labels_lose_windows_escapes_and_shortcuts() {
        assert_eq!(label_of("Tom && Jerry"), "Tom & Jerry");
        assert_eq!(label_of("New Window\tCtrl+N"), "New Window");
        assert_eq!(label_of("my_host"), "my__host", "an underscore is not a mnemonic");
        // Review M4: a single & is the mnemonic, dbusmenu's _.
        assert_eq!(label_of("&File"), "_File");
        assert_eq!(label_of("Save &As && Quit"), "Save _As & Quit");
        assert_eq!(label_of("a_&b"), "a___b");
        assert_eq!(label_of("end&"), "end");
    }

    /// Review I4: a row keeps its id across exports and a gone row's id is never reused, so a
    /// click on a menu shown before a rebuild runs the same command or nothing.
    #[test]
    fn ids_survive_a_rebuild() {
        let mut ids = Ids::default();
        let before = vec![row("Win 1", Kind::Item, "w1", vec![]), row("Win 2", Kind::Item, "w2", vec![]), row("", Kind::Separator, "s1", vec![]), row("New Window", Kind::Item, "new", vec![]), row("Quit", Kind::Item, "quit", vec![])];
        let t1 = flatten(&before, &mut ids);
        let id_of = |t: &BTreeMap<i32, Flat>, pick: &str| t.iter().find(|(_, f)| f.row.as_ref().is_some_and(|r| r.pick == pick)).map(|(id, _)| *id);
        let (w2, new, quit) = (id_of(&t1, "w2").unwrap(), id_of(&t1, "new").unwrap(), id_of(&t1, "quit").unwrap());
        // Window 2 closes; the separator's own muda id is new each build.
        let after = vec![row("Win 1", Kind::Item, "w1", vec![]), row("", Kind::Separator, "s2", vec![]), row("New Window", Kind::Item, "new", vec![]), row("Quit", Kind::Item, "quit", vec![])];
        let t2 = flatten(&after, &mut ids);
        assert_eq!(id_of(&t2, "new"), Some(new));
        assert_eq!(id_of(&t2, "quit"), Some(quit));
        assert!(!t2.contains_key(&w2), "the old row's id now names nothing (it was not Quit)");
        let t3 = flatten(&before, &mut ids);
        assert_ne!(id_of(&t3, "w2"), Some(w2), "a row that comes back gets a fresh id");
        assert_eq!(id_of(&t3, "quit"), Some(quit));
        // The same command twice gets two ids, both its own.
        let twice = vec![row("New Window", Kind::Item, "new", vec![]), row("Main Menu", Kind::Submenu, "m", vec![row("New Window", Kind::Item, "new", vec![])])];
        let t4 = flatten(&twice, &mut ids);
        assert_eq!(t4.values().filter(|f| f.row.as_ref().is_some_and(|r| r.pick == "new")).count(), 2);
        assert_eq!(t4.len(), 4);
    }

    #[test]
    fn the_icon_state_needs_the_watcher() {
        use crate::resident::Icon;
        assert_eq!(icon_of(false, false, true, false, false), Icon::Absent, "the watcher left: the icon is no way back");
        assert_eq!(icon_of(false, true, true, false, false), Icon::Up);
        assert_eq!(icon_of(false, true, false, true, false), Icon::Adding);
        assert_eq!(icon_of(false, true, false, false, false), Icon::Idle);
        assert_eq!(icon_of(false, true, false, false, true), Icon::Retrying, "a failed add between retries (N4)");
        assert_eq!(icon_of(false, true, false, true, true), Icon::Adding, "the retry is on its way");
        assert_eq!(icon_of(true, false, false, false, false), Icon::Unknown, "no first answer yet (N3)");
        assert_eq!(icon_of(true, true, false, false, false), Icon::Idle, "an answer that came wins");
    }

    #[test]
    fn rows_are_numbered_depth_first_under_the_root() {
        let rows = vec![row("New Window", Kind::Item, "a", vec![]), row("", Kind::Separator, "s", vec![]), row("Main Menu", Kind::Submenu, "m", vec![row("Copy", Kind::Item, "c", vec![]), row("Wrap", Kind::Check(true), "w", vec![])]), row("Quit", Kind::Item, "q", vec![])];
        let t = flatten(&rows, &mut Ids::default());
        assert_eq!(t.len(), 7);
        assert_eq!(t[&0].children, vec![1, 2, 3, 6]);
        assert_eq!(t[&3].children, vec![4, 5]);
        assert_eq!(t[&4].row.as_ref().unwrap().pick, "c");
        assert!(t[&3].row.as_ref().unwrap().children.is_empty(), "children live in the table, not the row");
        let s = |f: &Flat, k: &str| props(f, &[]).get(k).and_then(|v| <&str>::try_from(&**v).ok().map(str::to_string));
        assert_eq!(s(&t[&0], "children-display").as_deref(), Some("submenu"));
        assert_eq!(s(&t[&2], "type").as_deref(), Some("separator"));
        assert!(!props(&t[&2], &[]).contains_key("label"));
        assert_eq!(s(&t[&3], "children-display").as_deref(), Some("submenu"));
        assert_eq!(s(&t[&5], "toggle-type").as_deref(), Some("checkmark"));
        assert_eq!(props(&t[&5], &[]).get("toggle-state").and_then(|v| i32::try_from(&**v).ok()), Some(1));
        assert_eq!(props(&t[&1], &["label".to_string()]).len(), 1, "only the properties asked for");
    }

    #[test]
    fn muda_menus_become_rows() {
        let menu = muda::Menu::new();
        menu.append(&muda::MenuItem::with_id("a", "Tom && Jerry", true, None)).unwrap();
        menu.append(&muda::PredefinedMenuItem::separator()).unwrap();
        let sub = muda::Submenu::new("More", false);
        sub.append(&muda::CheckMenuItem::with_id("w", "Wrap", true, true, None)).unwrap();
        menu.append(&sub).unwrap();
        let rows = rows_of(&menu.items());
        assert_eq!(rows[0], row("Tom & Jerry", Kind::Item, "a", vec![]));
        assert_eq!(rows[1].kind, Kind::Separator);
        assert_eq!((rows[2].kind.clone(), rows[2].enabled), (Kind::Submenu, false));
        assert_eq!(rows[2].children[0].kind, Kind::Check(true));
    }

    /// A private session bus, killed with the guard.
    struct Bus {
        child: Child,
        addr: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn bus() -> Bus {
        let mut child = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().expect("dbus-daemon");
        let mut addr = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        Bus { child, addr: addr.trim().to_string() }
    }

    fn builder(addr: &str) -> impl FnOnce() -> zbus::Result<zbus::blocking::connection::Builder<'static>> + Send + 'static {
        let addr = addr.to_string();
        move || zbus::blocking::connection::Builder::address(addr.as_str())
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A stand-in watcher that records each registration.
    struct Watcher(Arc<Mutex<Vec<String>>>);

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(&self, service: String) {
            self.0.lock().unwrap().push(service);
        }
    }

    fn watcher(addr: &str) -> (Connection, Arc<Mutex<Vec<String>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let c = zbus::blocking::connection::Builder::address(addr).unwrap().name(WATCHER).unwrap().serve_at(WATCHER_PATH, Watcher(got.clone())).unwrap().build().unwrap();
        (c, got)
    }

    fn reset() {
        ADD_FAILED.store(false, Ordering::SeqCst);
        lock(&ITEM).take();
        *lock(&CLICK) = None;
        *lock(&PICK) = None;
        *lock(&HOST_BACK) = None;
        *lock(&LOST) = None;
        let mut m = lock(&MENU);
        m.table = BTreeMap::new();
        m.ids = Ids::default();
    }

    #[test]
    fn with_no_watcher_the_add_names_what_is_missing() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let b = bus();
        let e = add_with(builder(&b.addr)).unwrap_err();
        assert!(e.contains("StatusNotifierWatcher") && e.contains("AppIndicator"), "{e}");
        assert!(view().is_none(), "no icon, no anchor");
    }

    #[test]
    fn the_icon_registers_serves_its_menu_and_routes_clicks() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let b = bus();
        let (_w, registered) = watcher(&b.addr);
        let clicks = Arc::new(Mutex::new(Vec::new()));
        let picks = Arc::new(Mutex::new(Vec::new()));
        let menu = muda::Menu::new();
        menu.append(&muda::MenuItem::with_id("tray-new", "New Window", true, None)).unwrap();
        let sub = muda::Submenu::new("Main Menu", true);
        sub.append(&muda::MenuItem::with_id("copy", "Copy\tCtrl+Shift+C", true, None)).unwrap();
        sub.append(&muda::MenuItem::with_id("off", "Off", false, None)).unwrap();
        menu.append(&sub).unwrap();
        // The app's side: a request for the menu is answered with an export, as show_tray_menu does.
        let items = menu.items();
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let c2 = clicks.clone();
        on_click(Box::new(move |right| {
            c2.lock().unwrap().push(right);
            let _ = tx.send(right);
        }));
        let p2 = picks.clone();
        on_pick(Box::new(move |id| p2.lock().unwrap().push(id)));
        add_with(builder(&b.addr)).expect("added");
        assert_eq!(view(), Some(VIEW));
        assert!(registered.lock().unwrap()[0].starts_with("org.kde.StatusNotifierItem-"), "registered by its bus name");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true), "the first menu is asked for");
        export(&items);
        // The panel's side.
        let panel = zbus::blocking::connection::Builder::address(b.addr.as_str()).unwrap().build().unwrap();
        let name = registered.lock().unwrap()[0].clone();
        let prop = |iface: &str, path: &str, p: &str| -> OwnedValue {
            let r = panel.call_method(Some(name.as_str()), path, Some("org.freedesktop.DBus.Properties"), "Get", &(iface, p)).unwrap();
            r.body().deserialize::<OwnedValue>().unwrap()
        };
        assert_eq!(<&str>::try_from(&*prop("org.kde.StatusNotifierItem", ITEM_PATH, "Title")).unwrap(), "Termurion");
        assert_eq!(<&str>::try_from(&*prop("org.kde.StatusNotifierItem", ITEM_PATH, "Category")).unwrap(), "ApplicationStatus");
        assert!(!bool::try_from(&*prop("org.kde.StatusNotifierItem", ITEM_PATH, "ItemIsMenu")).unwrap());
        let pix = prop("org.kde.StatusNotifierItem", ITEM_PATH, "IconPixmap");
        let pix: Vec<(i32, i32, Vec<u8>)> = pix.try_clone().unwrap().try_into().unwrap();
        assert!(pix.iter().any(|(w, _, d)| *w == 32 && d.len() == 32 * 32 * 4));
        let r = panel.call_method(Some(name.as_str()), MENU_PATH, Some("com.canonical.dbusmenu"), "GetLayout", &(0i32, -1i32, Vec::<String>::new())).unwrap();
        let (_rev, (root, _props, kids)): (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)) = r.body().deserialize().unwrap();
        assert_eq!((root, kids.len()), (0, 2));
        let first = zbus::zvariant::Structure::try_from(kids[1].try_clone().unwrap()).unwrap();
        let fields = first.fields();
        let id = i32::try_from(&fields[0]).unwrap();
        let p: HashMap<String, OwnedValue> = HashMap::try_from(fields[1].try_clone().unwrap()).unwrap();
        assert_eq!(<&str>::try_from(&*p["label"]).unwrap(), "Main Menu");
        let Value::Array(sub_kids) = &fields[2] else { panic!("children") };
        assert_eq!(sub_kids.len(), 2, "the submenu's rows come with it");
        // Clicks: a row's muda id comes back; a submenu or a disabled row picks nothing.
        let event = |id: i32| panel.call_method(Some(name.as_str()), MENU_PATH, Some("com.canonical.dbusmenu"), "Event", &(id, "clicked", Value::from(0i32), 0u32)).unwrap();
        event(id + 1);
        event(id);
        event(id + 2);
        event(1);
        assert_eq!(*picks.lock().unwrap(), vec!["copy".to_string(), "tray-new".to_string()]);
        // A left click is the dock click.
        panel.call_method(Some(name.as_str()), ITEM_PATH, Some("org.kde.StatusNotifierItem"), "Activate", &(0i32, 0i32)).unwrap();
        wait_for("the left click", || clicks.lock().unwrap().contains(&false));
        // AboutToShow asks for a fresh menu; it waits for the export.
        let rows = rows_of(&items);
        let answer = std::thread::spawn(move || {
            // The left click above sent a false first: the ask is the true.
            while rx.recv_timeout(Duration::from_secs(5)) == Ok(false) {}
            export_rows(&rows);
        });
        let r = panel.call_method(Some(name.as_str()), MENU_PATH, Some("com.canonical.dbusmenu"), "AboutToShow", &(0i32,)).unwrap();
        assert!(r.body().deserialize::<bool>().unwrap(), "the panel is told to read it again");
        answer.join().unwrap();
        // Taking it down drops the bus name, which is what the watcher follows.
        set(false).unwrap();
        assert!(view().is_none());
        let dbus = zbus::blocking::fdo::DBusProxy::new(&panel).unwrap();
        wait_for("the name to go", || !dbus.name_has_owner(zbus::names::BusName::try_from(name.as_str()).unwrap()).unwrap());
    }

    /// A watcher that cannot take the icon (no RegisterStatusNotifierItem), for the failed add.
    struct Broken;

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Broken {
        fn other(&self) {}
    }

    /// Review I1/M5: the add runs off the UI thread; the icon counts as up only once the watcher
    /// took it, a failed add and a watcher that leaves are both told to the app.
    #[test]
    fn the_add_is_told_back_and_the_icon_follows_the_watcher() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let b = bus();
        let lost = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let l2 = lost.clone();
        on_lost(Box::new(move |why| l2.lock().unwrap().push(why)));
        let (w, registered) = watcher(&b.addr);
        watch_on(builder(&b.addr));
        wait_for("the watcher", host_up);
        assert_eq!(state(), crate::resident::Icon::Idle, "a panel, no icon yet");
        set_with(true, builder(&b.addr)).expect("the add started");
        wait_for("the icon up", || state() == crate::resident::Icon::Up);
        assert_eq!(registered.lock().unwrap().len(), 1);
        drop(w);
        wait_for("the watcher to leave", || state() == crate::resident::Icon::Absent);
        wait_for("the app told", || lost.lock().unwrap().contains(&None));
        assert!(set_with(true, builder(&b.addr)).is_err(), "no panel: the add says so at once");
        set_with(false, builder(&b.addr)).unwrap();
        // A watcher that refuses: the failure reaches the app, and nothing is up.
        let _broken = zbus::blocking::connection::Builder::address(b.addr.as_str()).unwrap().name(WATCHER).unwrap().serve_at(WATCHER_PATH, Broken).unwrap().build().unwrap();
        wait_for("the second watcher", host_up);
        set_with(true, builder(&b.addr)).expect("the add started");
        wait_for("the failure told", || lost.lock().unwrap().iter().any(|w| w.as_deref().is_some_and(|e| e.contains("not added"))));
        wait_for("the add ended", || state() == crate::resident::Icon::Retrying);
        drop(b);
        wait_for("the bus to end the watch", || !host_up());
        reset();
    }

    #[test]
    fn a_watcher_that_comes_back_asks_for_the_icon_again() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let b = bus();
        let back = Arc::new(AtomicU32::new(0));
        let b2 = back.clone();
        on_host_back(Box::new(move || {
            b2.fetch_add(1, Ordering::SeqCst);
        }));
        // A watcher there at the start is no comeback.
        let (w, _) = watcher(&b.addr);
        watch_on(builder(&b.addr));
        wait_for("the watcher", host_up);
        assert_eq!(back.load(Ordering::SeqCst), 0);
        drop(w);
        wait_for("the watcher to leave", || !host_up());
        let (_w, _) = watcher(&b.addr);
        wait_for("it came back", || back.load(Ordering::SeqCst) == 1);
        assert!(host_up());
        drop(b);
        wait_for("the bus to end the watch", || !host_up());
        reset();
    }
}
