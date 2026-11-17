//! File search with the prefix "%" ("%invoice 2024"): file and folder names from the
//! Windows Search index, the same index Explorer's search box uses. Only what Windows
//! indexes is found (by default the user's folders, Start menu, ...).
//!
//! Queries go through OLE DB to the index (provider Search.CollatorDSO, Windows
//! Search SQL), on a worker thread that keeps its connection open and always works
//! on the newest text only: typing fast never queues up queries. Each word must start
//! a word of the file name (`CONTAINS(System.FileName, '"rech*" AND "2024*"')`), newest
//! files first. Results (with icons) are posted to the window.

use std::mem::{offset_of, ManuallyDrop};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use windows::core::{IUnknown, Interface, GUID, HSTRING};
use windows::Win32::Foundation::SIZE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Search::*;
use windows::Win32::UI::Shell::*;

/// Results per query (the list shows 8 and scrolls).
pub const MAX_RESULTS: usize = 30;
/// SQL dialect GUID of OLE DB (DBGUID_DEFAULT; not exported by the windows crate).
const DBGUID_DEFAULT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);
const CONNECTION: &str = "Provider=Search.CollatorDSO;Extended Properties='Application=Windows';";

/// One found file or folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub name: String,
    pub path: String,
    pub folder: bool,
    /// Premultiplied HBITMAP (0: none).
    pub icon: isize,
}

/// Results for one query, identified by its number.
pub struct Results {
    pub generation: u64,
    pub hits: Vec<Hit>,
}

struct Query {
    generation: u64,
    text: String,
    icon_px: i32,
    notify: usize,
    msg: u32,
}

static SENDER: OnceLock<Mutex<Sender<Query>>> = OnceLock::new();
/// The newest results, taken by the window when it gets `msg`.
static READY: Mutex<Option<Results>> = Mutex::new(None);

/// Starts a search for `text` (without the prefix). `msg` is posted to `notify` when
/// its results are ready; then `take_results` returns them.
pub fn search(generation: u64, text: &str, icon_px: i32, notify: usize, msg: u32) {
    let sender = SENDER.get_or_init(|| {
        let (tx, rx) = channel();
        std::thread::spawn(move || worker(rx));
        Mutex::new(tx)
    });
    let _ = sender.lock().unwrap().send(Query {
        generation,
        text: text.to_string(),
        icon_px,
        notify,
        msg,
    });
}

/// The results posted last, if any.
pub fn take_results() -> Option<Results> {
    READY.lock().unwrap().take()
}

fn worker(rx: Receiver<Query>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let mut session: Option<IDBCreateCommand> = None;
    while let Ok(mut query) = rx.recv() {
        // Skip to the newest text typed meanwhile.
        while let Ok(newer) = rx.try_recv() {
            query = newer;
        }
        let hits = match build_sql(&query.text, MAX_RESULTS) {
            None => Vec::new(),
            Some(sql) => {
                let mut result = unsafe { run(&mut session, &sql) };
                if result.is_err() {
                    // The index service may have restarted: reconnect once.
                    session = None;
                    result = unsafe { run(&mut session, &sql) };
                }
                result.unwrap_or_else(|e| {
                    crate::debug_log::log(&format!("file search failed: {e}"));
                    Vec::new()
                })
            }
        };
        let hits = hits
            .into_iter()
            .map(|mut hit| {
                hit.icon = unsafe { icon(&hit.path, query.icon_px) };
                hit
            })
            .collect();
        // Results nobody took (the window moved on) give their icons back.
        if let Some(old) = READY.lock().unwrap().replace(Results {
            generation: query.generation,
            hits,
        }) {
            free_icons(&old.hits);
        }
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                query.notify as _,
                query.msg,
                0,
                0,
            );
        }
    }
}

/// Frees the icons of results that are no longer shown.
pub fn free_icons(hits: &[Hit]) {
    for hit in hits.iter().filter(|h| h.icon != 0) {
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(hit.icon as _) };
    }
}

/// Windows Search SQL for the typed words; `None` if nothing searchable is left.
/// Only letters, digits and `. - _` survive, so the text cannot break out of the
/// query.
pub fn build_sql(text: &str, limit: usize) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    let condition = words
        .iter()
        .map(|w| format!("\"{w}*\""))
        .collect::<Vec<_>>()
        .join(" AND ");
    Some(format!(
        "SELECT TOP {limit} System.ItemNameDisplay, System.ItemUrl, System.ItemType \
         FROM SystemIndex WHERE SCOPE='file:' AND CONTAINS(System.FileName, '{condition}') \
         ORDER BY System.DateModified DESC"
    ))
}

/// "file:C:/Users/x/a b.pdf" -> "C:\Users\x\a b.pdf".
pub fn path_from_url(url: &str) -> Option<String> {
    let path = url.strip_prefix("file:")?;
    Some(path.replace('/', "\\"))
}

/// Row buffer for the accessor: three strings, allocated by the provider.
#[repr(C)]
struct Row {
    name: *mut u16,
    name_status: u32,
    url: *mut u16,
    url_status: u32,
    kind: *mut u16,
    kind_status: u32,
}

const DBSTATUS_S_OK: u32 = 0;

unsafe fn take_string(p: *mut u16, status: u32) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = (status == DBSTATUS_S_OK).then(|| {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    });
    CoTaskMemFree(Some(p as _));
    s
}

/// Runs one query; opens the connection first if needed.
unsafe fn run(
    session: &mut Option<IDBCreateCommand>,
    sql: &str,
) -> windows::core::Result<Vec<Hit>> {
    if session.is_none() {
        let init: IDataInitialize = CoCreateInstance(&MSDAINITIALIZE, None, CLSCTX_INPROC_SERVER)?;
        let mut source: Option<IUnknown> = None;
        init.GetDataSource(
            None,
            CLSCTX_INPROC_SERVER.0,
            &HSTRING::from(CONNECTION),
            &IDBInitialize::IID,
            &mut source,
        )?;
        let source: IDBInitialize = source.ok_or(windows::core::Error::empty())?.cast()?;
        source.Initialize()?;
        let create: IDBCreateSession = source.cast()?;
        *session = Some(create.CreateSession(None, &IDBCreateCommand::IID)?.cast()?);
    }
    let create_command = session.as_ref().ok_or(windows::core::Error::empty())?;
    let command: ICommandText = create_command
        .CreateCommand(None, &ICommandText::IID)?
        .cast()?;
    command.SetCommandText(&DBGUID_DEFAULT, &HSTRING::from(sql))?;
    let mut rowset: Option<IUnknown> = None;
    command.Execute(None, &IRowset::IID, None, None, Some(&mut rowset))?;
    let rowset: IRowset = rowset.ok_or(windows::core::Error::empty())?.cast()?;
    let accessor: IAccessor = rowset.cast()?;

    let binding = |ordinal: usize, value: usize, status: usize| DBBINDING {
        iOrdinal: ordinal,
        obValue: value,
        obLength: 0,
        obStatus: status,
        pTypeInfo: ManuallyDrop::new(None),
        pObject: std::ptr::null_mut(),
        pBindExt: std::ptr::null_mut(),
        dwPart: (DBPART_VALUE.0 | DBPART_STATUS.0) as u32,
        dwMemOwner: DBMEMOWNER_CLIENTOWNED.0 as u32,
        eParamIO: DBPARAMIO_NOTPARAM.0 as u32,
        cbMaxLen: 0,
        dwFlags: 0,
        wType: (DBTYPE_WSTR.0 | DBTYPE_BYREF.0) as u16,
        bPrecision: 0,
        bScale: 0,
    };
    let bindings = [
        binding(1, offset_of!(Row, name), offset_of!(Row, name_status)),
        binding(2, offset_of!(Row, url), offset_of!(Row, url_status)),
        binding(3, offset_of!(Row, kind), offset_of!(Row, kind_status)),
    ];
    let mut handle = HACCESSOR::default();
    accessor.CreateAccessor(
        DBACCESSOR_ROWDATA.0 as u32,
        bindings.len(),
        bindings.as_ptr(),
        std::mem::size_of::<Row>(),
        &mut handle,
        None,
    )?;

    let mut hits = Vec::new();
    loop {
        let mut row_handle = 0usize;
        let mut rows = [&mut row_handle as *mut usize];
        let mut obtained = 0usize;
        if rowset.GetNextRows(0, 0, &mut obtained, &mut rows).is_err() || obtained == 0 {
            break;
        }
        let mut row: Row = std::mem::zeroed();
        let got = rowset.GetData(row_handle, handle, &mut row as *mut Row as _);
        let _ = rowset.ReleaseRows(
            1,
            &row_handle,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if got.is_err() {
            continue;
        }
        let name = take_string(row.name, row.name_status);
        let url = take_string(row.url, row.url_status);
        let kind = take_string(row.kind, row.kind_status);
        if let (Some(name), Some(path)) = (name, url.as_deref().and_then(path_from_url)) {
            hits.push(Hit {
                name,
                path,
                folder: kind.as_deref() == Some("Directory"),
                icon: 0,
            });
        }
    }
    let _ = accessor.ReleaseAccessor(handle, None);
    Ok(hits)
}

/// The icon Explorer shows for a path, as a premultiplied HBITMAP (0: none).
unsafe fn icon(path: &str, icon_px: i32) -> isize {
    if icon_px <= 0 {
        return 0;
    }
    let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path), None)
    else {
        return 0;
    };
    let Ok(factory) = item.cast::<IShellItemImageFactory>() else {
        return 0;
    };
    factory
        .GetImage(
            SIZE {
                cx: icon_px,
                cy: icon_px,
            },
            SIIGBF_ICONONLY,
        )
        .map_or(0, |b| {
            crate::icon_cache::premultiply_bitmap(b.0 as isize);
            b.0 as isize
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_is_safe_and_matches_word_starts() {
        let sql = build_sql("Rech 2024", 30).unwrap();
        assert!(
            sql.contains(r#"CONTAINS(System.FileName, '"Rech*" AND "2024*"')"#),
            "{sql}"
        );
        assert!(sql.starts_with("SELECT TOP 30 "));
        let sql = build_sql("a'; DROP \"x\" %", 5).unwrap();
        assert!(sql.contains(r#"'"a*" AND "DROP*" AND "x*"'"#), "{sql}");
        assert_eq!(build_sql("  '\"%  ", 5), None);
        assert!(build_sql("bericht_v2.pdf", 5)
            .unwrap()
            .contains(r#""bericht_v2.pdf*""#));
    }

    #[test]
    fn paths_from_urls() {
        assert_eq!(
            path_from_url("file:C:/Users/x/a b.pdf").as_deref(),
            Some(r"C:\Users\x\a b.pdf")
        );
        assert_eq!(path_from_url("mapi:x"), None);
    }

    /// Real query against the index: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn queries_the_real_index() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let mut session = None;
        let t = std::time::Instant::now();
        let hits = unsafe { run(&mut session, &build_sql("downl", 10).unwrap()) }.expect("query");
        println!("{} hits in {:?}", hits.len(), t.elapsed());
        for h in &hits {
            println!("   {} | {} | folder={}", h.name, h.path, h.folder);
        }
        let t = std::time::Instant::now();
        let again = unsafe { run(&mut session, &build_sql("readme", 10).unwrap()) }.expect("query");
        println!("second query: {} hits in {:?}", again.len(), t.elapsed());
        assert!(!hits.is_empty());
    }
}
