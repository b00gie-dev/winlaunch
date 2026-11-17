//! Icon cache on disk, so the app icons are there with the names right after startup
//! instead of about a second later (loading ~150 icons through the shell is the slow
//! part of the app scan).
//!
//! `%LOCALAPPDATA%\WinLaunch\icons.bin` holds the finished pixels per target (app ID or
//! path) for one icon size. Each entry carries a stamp: for targets that are files
//! (programs, shortcuts) their modification time and size, so an updated program
//! gets its new icon; for everything else (Store apps) the time it was cached, and
//! such entries are reloaded after 7 days. A different icon size (display scaling) or
//! file version discards the whole cache. The file is written atomically, and only
//! when something changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 4] = b"WLIC";
/// 2: pixels are premultiplied (version 1 stored the shell's straight alpha).
const VERSION: u32 = 2;
/// Entries without a file stamp (Store apps) are reloaded after this long.
const MAX_AGE_WITHOUT_STAMP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Sanity limit for one icon's side length in pixels.
const MAX_SIDE: i32 = 1024;

/// One cached icon: 32-bit premultiplied BGRA pixels as in the DIB section.
#[derive(Clone, Debug, PartialEq)]
pub struct Cached {
    /// File stamp of the target, 0 if it is not a file (see `stamp`).
    pub stamp: u64,
    /// When the icon was loaded from the shell (unix seconds).
    pub cached_at: u64,
    pub width: i32,
    /// Negative for top-down rows, as in BITMAPINFOHEADER.
    pub height: i32,
    pub pixels: Vec<u8>,
}

impl Cached {
    /// True if the entry still shows the current icon of a target with `stamp`.
    pub fn is_current(&self, stamp: u64, now: u64) -> bool {
        if stamp != 0 {
            return self.stamp == stamp;
        }
        self.stamp == 0 && now.saturating_sub(self.cached_at) < MAX_AGE_WITHOUT_STAMP.as_secs()
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn path() -> Option<PathBuf> {
    Some(crate::local_data_dir()?.join("icons.bin"))
}

/// Stamp of a target: modification time and size of the file it names, or 0.
/// AppsFolder targets of classic programs look like
/// `{6D809377-6AF0-444B-8957-A3773F02200E}\OrcaSlicer\orca-slicer.exe` (a known folder
/// plus a path); Store apps are app IDs like `Microsoft.WindowsCalculator_…!App`.
pub fn stamp(target: &str) -> u64 {
    let Some(path) = file_path(target) else {
        return 0;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return 0;
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    // Never 0 for an existing file, so it is not mistaken for "no file".
    (modified.rotate_left(20) ^ meta.len()) | 1
}

/// The file a target names, with a leading `{known folder GUID}` resolved.
fn file_path(target: &str) -> Option<String> {
    if let Some(rest) = target.strip_prefix('{') {
        let (guid, tail) = rest.split_once('}')?;
        let guid = windows::core::GUID::try_from(guid).ok()?;
        let base = unsafe {
            use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
            let p = SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
            s?
        };
        return Some(format!("{base}{tail}"));
    }
    let looks_like_path =
        target.len() > 2 && target.as_bytes()[1] == b':' || target.starts_with("\\\\");
    looks_like_path.then(|| target.to_string())
}

/// Serializes the cache: header (magic, version, icon size, count), then per entry
/// key length + key (UTF-8), stamp, cached_at, width, height, pixel length + pixels.
pub fn encode(icon_px: i32, entries: &[(&str, &Cached)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&icon_px.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (key, c) in entries {
        out.extend_from_slice(&(key.len() as u32).to_le_bytes());
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(&c.stamp.to_le_bytes());
        out.extend_from_slice(&c.cached_at.to_le_bytes());
        out.extend_from_slice(&c.width.to_le_bytes());
        out.extend_from_slice(&c.height.to_le_bytes());
        out.extend_from_slice(&(c.pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&c.pixels);
    }
    out
}

/// Reads a cache written by `encode`; `None` if it is damaged, of another version or
/// for another icon size.
pub fn decode(data: &[u8], icon_px: i32) -> Option<HashMap<String, Cached>> {
    let mut r = Reader { data, pos: 0 };
    if r.bytes(4)? != MAGIC || r.u32()? != VERSION || r.i32()? != icon_px {
        return None;
    }
    let count = r.u32()?;
    let mut map = HashMap::new();
    for _ in 0..count {
        let key_len = r.u32()? as usize;
        let key = String::from_utf8(r.bytes(key_len)?.to_vec()).ok()?;
        let stamp = r.u64()?;
        let cached_at = r.u64()?;
        let width = r.i32()?;
        let height = r.i32()?;
        let len = r.u32()? as usize;
        let ok_size = (1..=MAX_SIDE).contains(&width)
            && (1..=MAX_SIDE).contains(&height.abs())
            && len == (width * height.abs() * 4) as usize;
        if !ok_size {
            return None;
        }
        let pixels = r.bytes(len)?.to_vec();
        map.insert(
            key,
            Cached {
                stamp,
                cached_at,
                width,
                height,
                pixels,
            },
        );
    }
    (r.pos == data.len()).then_some(map)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let b = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }
}

/// The cache on disk for `icon_px` (empty if missing or unusable).
pub fn load(icon_px: i32) -> HashMap<String, Cached> {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|data| decode(&data, icon_px))
        .unwrap_or_default()
}

/// Writes the cache atomically (temporary file, then rename).
pub fn save(icon_px: i32, entries: &[(&str, &Cached)]) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("bin.tmp");
    if std::fs::write(&tmp, encode(icon_px, entries)).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Pixels of an icon bitmap from the shell (a 32-bit DIB section). `None` for other
/// bitmaps. Reads the section's memory directly, which is safe even while the window
/// paints with it (unlike GetDIBits).
pub unsafe fn pixels_of(hbitmap: isize) -> Option<(i32, i32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{GetObjectW, DIBSECTION};
    let mut ds: DIBSECTION = std::mem::zeroed();
    let size = std::mem::size_of::<DIBSECTION>() as i32;
    if GetObjectW(hbitmap as _, size, &mut ds as *mut DIBSECTION as _) != size
        || ds.dsBm.bmBits.is_null()
        || ds.dsBm.bmBitsPixel != 32
    {
        return None;
    }
    let width = ds.dsBmih.biWidth;
    let height = ds.dsBmih.biHeight;
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height.abs()) {
        return None;
    }
    let len = (width * height.abs() * 4) as usize;
    let pixels = std::slice::from_raw_parts(ds.dsBm.bmBits as *const u8, len).to_vec();
    Some((width, height, pixels))
}

/// Turns straight alpha into premultiplied alpha, in place, if the pixels need it.
///
/// `IShellItemImageFactory::GetImage` hands out 32-bit bitmaps with *straight* alpha
/// (measured: most semi-transparent edge pixels have colour values above their alpha),
/// while `AlphaBlend` with `AC_SRC_ALPHA` expects premultiplied pixels. Drawn as they
/// are, anti-aliased edges come out too bright and look jagged. A pixel with a colour
/// value above its alpha is impossible in premultiplied data, so only such bitmaps are
/// converted; already premultiplied ones are left alone.
pub fn premultiply(pixels: &mut [u8]) {
    let straight = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    if !straight {
        return;
    }
    for p in pixels.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

/// `premultiply` applied to a 32-bit DIB section from the shell (in its own memory).
pub unsafe fn premultiply_bitmap(hbitmap: isize) {
    use windows_sys::Win32::Graphics::Gdi::{GdiFlush, GetObjectW, DIBSECTION};
    let mut ds: DIBSECTION = std::mem::zeroed();
    let size = std::mem::size_of::<DIBSECTION>() as i32;
    if GetObjectW(hbitmap as _, size, &mut ds as *mut DIBSECTION as _) != size
        || ds.dsBm.bmBits.is_null()
        || ds.dsBm.bmBitsPixel != 32
    {
        return;
    }
    GdiFlush(); // finish pending GDI drawing into the section before touching it
    let len = (ds.dsBm.bmWidthBytes * ds.dsBm.bmHeight) as usize;
    premultiply(std::slice::from_raw_parts_mut(
        ds.dsBm.bmBits as *mut u8,
        len,
    ));
}

/// A DIB section with the cached pixels, like the ones the shell hands out.
pub unsafe fn to_bitmap(c: &Cached) -> isize {
    use windows_sys::Win32::Graphics::Gdi::*;
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = c.width;
    bmi.bmiHeader.biHeight = c.height;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(
        std::ptr::null_mut(),
        &bmi,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if bmp.is_null() || bits.is_null() {
        return 0;
    }
    std::ptr::copy_nonoverlapping(c.pixels.as_ptr(), bits as *mut u8, c.pixels.len());
    bmp as isize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(stamp: u64, cached_at: u64) -> Cached {
        Cached {
            stamp,
            cached_at,
            width: 2,
            height: -2,
            pixels: (0..16).collect(),
        }
    }

    #[test]
    fn premultiplies_straight_alpha_only() {
        // Straight: half-transparent white (255 > 128) and a transparent pixel.
        let mut straight = vec![255, 255, 255, 128, 0, 0, 0, 0];
        premultiply(&mut straight);
        assert_eq!(straight, [128, 128, 128, 128, 0, 0, 0, 0]);
        // Already premultiplied: unchanged.
        let mut premultiplied = vec![100, 50, 128, 128, 255, 255, 255, 255];
        premultiply(&mut premultiplied);
        assert_eq!(premultiplied, [100, 50, 128, 128, 255, 255, 255, 255]);
    }

    #[test]
    fn round_trip() {
        let a = icon(7, 100);
        let b = icon(0, 200);
        let data = encode(24, &[("C:\\a.exe", &a), ("Store.App!App", &b)]);
        let map = decode(&data, 24).unwrap();
        assert_eq!(map["C:\\a.exe"], a);
        assert_eq!(map["Store.App!App"], b);
    }

    #[test]
    fn rejects_other_size_damage_and_garbage() {
        let data = encode(24, &[("k", &icon(1, 1))]);
        assert_eq!(decode(&data, 36), None, "other display scaling");
        assert_eq!(decode(&data[..data.len() - 1], 24), None, "truncated");
        let mut longer = data.clone();
        longer.push(0);
        assert_eq!(decode(&longer, 24), None, "trailing bytes");
        assert_eq!(decode(b"nonsense", 24), None);
        let mut wrong_len = icon(1, 1);
        wrong_len.pixels.pop();
        assert_eq!(decode(&encode(24, &[("k", &wrong_len)]), 24), None);
    }

    #[test]
    fn currency_rules() {
        let now = 10 * 24 * 60 * 60;
        assert!(icon(5, 0).is_current(5, now), "same file stamp");
        assert!(!icon(5, 0).is_current(6, now), "program updated");
        assert!(icon(0, now - 60).is_current(0, now), "store app, fresh");
        assert!(
            !icon(0, 0).is_current(0, now),
            "store app, older than 7 days"
        );
        assert!(!icon(5, now).is_current(0, now), "file vanished");
    }

    #[test]
    fn stamps_files_only() {
        let exe = std::env::current_exe().unwrap();
        assert_ne!(stamp(&exe.to_string_lossy()), 0);
        assert_eq!(stamp("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), 0);
        assert_eq!(stamp(r"C:\does\not\exist.exe"), 0);
        // FOLDERID_Windows + \explorer.exe
        assert_ne!(
            stamp(r"{F38BF404-1D43-42F2-9305-67DE0B28FC23}\explorer.exe"),
            0
        );
    }
}
