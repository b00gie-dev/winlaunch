//! Currency converter: "100 usd in eur", "100$", "50 chf", "€ 20 to £". The result
//! uses the calculator row; Enter copies the converted amount.
//!
//! Rates are the European Central Bank's daily reference rates (about 30 currencies,
//! published once per working day around 16:00 CET, no account or key needed). They
//! are fetched only when a conversion is typed and the saved ones are older than a
//! day, then kept in `%LOCALAPPDATA%\WinLaunch\rates.xml`, so conversions work offline
//! and right after startup (showing the date of the rates).

use crate::calc::{format_fixed, Answer};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const HOST: &str = "www.ecb.europa.eu";
const PATH: &str = "/stats/eurofxref/eurofxref-daily.xml";
/// Saved rates older than this are refreshed (the ECB publishes once a day).
const REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed download, wait this long before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// ... or this long when there are no rates at all yet.
const RETRY_WITHOUT_RATES: Duration = Duration::from_secs(60);
/// Icon of the currency row ("Segoe MDL2 Assets": Money).
const GLYPH: char = '\u{E1D0}';

/// Currencies of the ECB reference rates (plus the euro itself).
const CODES: &[&str] = &[
    "EUR", "USD", "JPY", "BGN", "CZK", "DKK", "GBP", "HUF", "PLN", "RON", "SEK", "CHF", "ISK",
    "NOK", "TRY", "AUD", "BRL", "CAD", "CNY", "HKD", "IDR", "ILS", "INR", "KRW", "MXN", "MYR",
    "NZD", "PHP", "SGD", "THB", "ZAR",
];

/// Names and symbols people type instead of the code.
const NAMES: &[(&str, &str)] = &[
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("dollar", "USD"),
    ("dollars", "USD"),
    ("euro", "EUR"),
    ("euros", "EUR"),
    ("pfund", "GBP"),
    ("pound", "GBP"),
    ("pounds", "GBP"),
    ("franken", "CHF"),
    ("franc", "CHF"),
    ("francs", "CHF"),
    ("yen", "JPY"),
    ("yuan", "CNY"),
    ("renminbi", "CNY"),
    ("zloty", "PLN"),
    ("rand", "ZAR"),
    ("rupie", "INR"),
    ("rupee", "INR"),
    ("rupees", "INR"),
    ("won", "KRW"),
    ("lira", "TRY"),
];

/// Words between the amount and the target currency.
const CONNECTORS: &[&str] = &["in", "to", "into", "nach", "zu", "as", "=", "->"];

/// Rates relative to the euro, from one ECB file.
#[derive(Debug, PartialEq)]
pub struct Rates {
    /// Date the rates are for, as in the file ("2026-11-13").
    pub date: String,
    per_euro: HashMap<String, f64>,
}

impl Rates {
    fn per_euro(&self, code: &str) -> Option<f64> {
        if code == "EUR" {
            Some(1.0)
        } else {
            self.per_euro.get(code).copied()
        }
    }

    /// `amount` of `from` in `to`.
    pub fn convert(&self, amount: f64, from: &str, to: &str) -> Option<f64> {
        Some(amount / self.per_euro(from)? * self.per_euro(to)?)
    }
}

/// Reads the ECB's eurofxref-daily.xml: `<Cube time='…'>` and
/// `<Cube currency='USD' rate='1.0841'/>` elements.
pub fn parse_ecb(xml: &str) -> Option<Rates> {
    let mut date = None;
    let mut per_euro = HashMap::new();
    for tag in xml.split('<').filter(|t| t.starts_with("Cube")) {
        if let Some(time) = attribute(tag, "time") {
            date = Some(time.to_string());
        }
        if let (Some(code), Some(rate)) = (attribute(tag, "currency"), attribute(tag, "rate")) {
            if let Ok(rate) = rate.parse::<f64>() {
                if rate > 0.0 {
                    per_euro.insert(code.to_uppercase(), rate);
                }
            }
        }
    }
    (!per_euro.is_empty()).then(|| Rates {
        date: date.unwrap_or_default(),
        per_euro,
    })
}

/// Value of `name='…'` or `name="…"` in a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag[start..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &tag[start + 1..];
    Some(&rest[..rest.find(quote)?])
}

/// What was typed: amount, source currency and, if given, the target currency.
#[derive(Debug, PartialEq)]
pub struct Conversion {
    pub amount: f64,
    pub from: &'static str,
    pub to: Option<&'static str>,
}

#[derive(Debug, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.trim().to_lowercase().chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || ((c == '.' || c == ',') && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | ',')) {
                i += 1;
            }
            let number: String = chars[start..i].iter().collect::<String>().replace(',', ".");
            tokens.push(Token::Number(number.parse().ok()?));
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            tokens.push(Token::Word(chars[start..i].iter().collect()));
        } else if c == '-' && chars.get(i + 1) == Some(&'>') {
            tokens.push(Token::Word("->".into()));
            i += 2;
        } else if matches!(c, '$' | '€' | '£' | '¥' | '=') {
            tokens.push(Token::Word(c.to_string()));
            i += 1;
        } else {
            return None;
        }
    }
    Some(tokens)
}

/// Currency code for a typed code, name or symbol.
fn currency(word: &str) -> Option<&'static str> {
    let upper = word.to_uppercase();
    CODES
        .iter()
        .find(|c| **c == upper)
        .copied()
        .or_else(|| NAMES.iter().find(|(n, _)| *n == word).map(|(_, c)| *c))
}

/// "100 usd", "$100", "100 usd in eur", "100 usd eur", "€20 to £".
pub fn parse(text: &str) -> Option<Conversion> {
    let tokens = tokenize(text)?;
    let word_currency = |t: &Token| match t {
        Token::Word(w) => currency(w),
        Token::Number(_) => None,
    };
    let (amount, from) = match tokens.as_slice() {
        [Token::Number(n), c, ..] => (*n, word_currency(c)?),
        [c, Token::Number(n), ..] => (*n, word_currency(c)?),
        _ => return None,
    };
    let to = match &tokens[2..] {
        [] => None,
        [c] => Some(word_currency(c)?),
        [Token::Word(w), c] if CONNECTORS.contains(&w.as_str()) => Some(word_currency(c)?),
        _ => return None,
    };
    Some(Conversion { amount, from, to })
}

/// Euro to dollar, everything else to euro, when no target is typed.
fn default_target(from: &str) -> &'static str {
    if from == "EUR" {
        "USD"
    } else {
        "EUR"
    }
}

/// Saved and downloaded rates.
struct Store {
    rates: Option<Arc<Rates>>,
    /// When the rates were fetched (the file's modification time).
    fetched: Option<SystemTime>,
    read_file: bool,
    fetching: bool,
    last_attempt: Option<Instant>,
    failed: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    rates: None,
    fetched: None,
    read_file: false,
    fetching: false,
    last_attempt: None,
    failed: false,
});

fn path() -> Option<PathBuf> {
    Some(crate::local_data_dir()?.join("rates.xml"))
}

/// Current rates; reads the saved file on first use and starts a background download
/// when they are missing or older than a day. `notify`/`msg` is posted when new rates
/// (or a failure) arrive, so the open launcher can show them.
fn rates(notify: usize, msg: u32) -> (Option<Arc<Rates>>, bool) {
    let mut store = STORE.lock().unwrap();
    if !store.read_file {
        store.read_file = true;
        if let Some(p) = path() {
            if let Ok(xml) = std::fs::read_to_string(&p) {
                store.rates = parse_ecb(&xml).map(Arc::new);
                store.fetched = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            }
        }
    }
    let stale = store
        .fetched
        .and_then(|f| f.elapsed().ok())
        .is_none_or(|age| age > REFRESH_AFTER);
    let retry = if store.rates.is_some() {
        RETRY_AFTER
    } else {
        RETRY_WITHOUT_RATES
    };
    let may_try = store.last_attempt.is_none_or(|t| t.elapsed() > retry);
    if stale && may_try && !store.fetching {
        store.fetching = true;
        store.last_attempt = Some(Instant::now());
        std::thread::spawn(move || download(notify, msg));
    }
    (store.rates.clone(), store.failed && store.rates.is_none())
}

fn download(notify: usize, msg: u32) {
    let result = crate::net::get(HOST, PATH).and_then(|body| {
        let xml = String::from_utf8(body).map_err(|e| e.to_string())?;
        let rates = parse_ecb(&xml).ok_or("no rates in the file")?;
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = p.with_extension("xml.tmp");
            if std::fs::write(&tmp, &xml).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
        Ok(rates)
    });
    {
        let mut store = STORE.lock().unwrap();
        store.fetching = false;
        match result {
            Ok(rates) => {
                store.rates = Some(Arc::new(rates));
                store.fetched = Some(SystemTime::now());
                store.failed = false;
            }
            Err(e) => {
                crate::debug_log::log(&format!("currency rates: {e}"));
                store.failed = true;
            }
        }
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(notify as _, msg, 0, 0);
    }
}

/// The converter row for `text`, if it is a conversion.
pub fn answer(text: &str, language: &str, notify: usize, msg: u32) -> Option<Answer> {
    let conversion = parse(text)?;
    let to = conversion.to.unwrap_or(default_target(conversion.from));
    if to == conversion.from {
        return None;
    }
    let label = crate::i18n::t("result-kind-currency");
    let (rates, failed) = rates(notify, msg);
    let Some(rates) = rates else {
        let display = crate::i18n::t(if failed {
            "currency-unavailable"
        } else {
            "currency-loading"
        });
        return Some(Answer {
            display,
            copy: String::new(),
            label: Some(label),
            glyph: GLYPH,
        });
    };
    let result = rates.convert(conversion.amount, conversion.from, to)?;
    let (decimal, group) = if language == "de" {
        (',', '.')
    } else {
        ('.', ',')
    };
    let amount = format_fixed(conversion.amount, 2, decimal, Some(group));
    let converted = format_fixed(result, 2, decimal, Some(group));
    let date = match (
        language,
        rates.date.split('-').collect::<Vec<_>>().as_slice(),
    ) {
        ("de", [y, m, d]) => format!("{d}.{m}.{y}"),
        _ => rates.date.clone(),
    };
    Some(Answer {
        display: format!("{amount} {} = {converted} {to}", conversion.from),
        copy: format_fixed(result, 2, decimal, None),
        label: Some(crate::i18n::t_args("currency-source", &[("date", &date)])),
        glyph: GLYPH,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01">
  <Cube>
    <Cube time='2026-11-13'>
      <Cube currency='USD' rate='1.0800'/>
      <Cube currency='GBP' rate='0.8600'/>
      <Cube currency='CHF' rate='0.9400'/>
    </Cube>
  </Cube>
</gesmes:Envelope>"#;

    #[test]
    fn reads_ecb_file() {
        let rates = parse_ecb(XML).unwrap();
        assert_eq!(rates.date, "2026-11-13");
        assert_eq!(rates.per_euro("USD"), Some(1.08));
        assert_eq!(rates.per_euro("EUR"), Some(1.0));
        assert_eq!(parse_ecb("<html>maintenance</html>"), None);
    }

    #[test]
    fn converts_through_the_euro() {
        let rates = parse_ecb(XML).unwrap();
        assert!((rates.convert(108.0, "USD", "EUR").unwrap() - 100.0).abs() < 1e-9);
        assert!((rates.convert(100.0, "EUR", "GBP").unwrap() - 86.0).abs() < 1e-9);
        assert!((rates.convert(94.0, "CHF", "USD").unwrap() - 108.0).abs() < 1e-9);
        assert_eq!(rates.convert(1.0, "USD", "JPY"), None, "not in this file");
    }

    #[test]
    fn understands_typical_input() {
        let c = |amount, from, to| Some(Conversion { amount, from, to });
        assert_eq!(parse("100 usd"), c(100.0, "USD", None));
        assert_eq!(parse("100usd"), c(100.0, "USD", None));
        assert_eq!(parse("$100"), c(100.0, "USD", None));
        assert_eq!(parse("100$"), c(100.0, "USD", None));
        assert_eq!(parse("12,50 € in £"), c(12.5, "EUR", Some("GBP")));
        assert_eq!(parse("100 USD to EUR"), c(100.0, "USD", Some("EUR")));
        assert_eq!(parse("50 chf nach eur"), c(50.0, "CHF", Some("EUR")));
        assert_eq!(parse("20 dollar euro"), c(20.0, "USD", Some("EUR")));
        assert_eq!(parse("5 pfund -> franken"), c(5.0, "GBP", Some("CHF")));
    }

    #[test]
    fn ignores_everything_else() {
        for s in [
            "",
            "100",
            "usd",
            "chrome",
            "7 days to die",
            "100 abc",
            "100 usd in abc",
            "1+2",
            "100 usd eur gbp",
            "Windows 11",
        ] {
            assert_eq!(parse(s), None, "{s}");
        }
    }

    /// Real download from the ECB (network): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_real_rates() {
        let body = crate::net::get(HOST, PATH).expect("download");
        let rates = parse_ecb(std::str::from_utf8(&body).unwrap()).expect("rates");
        assert!(rates.per_euro("USD").is_some() && rates.per_euro.len() > 20);
        println!("{} currencies, date {}", rates.per_euro.len(), rates.date);
    }

    #[test]
    fn default_targets() {
        assert_eq!(default_target("USD"), "EUR");
        assert_eq!(default_target("EUR"), "USD");
    }
}
