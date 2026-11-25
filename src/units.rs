//! Unit converter: "100 ps in kw", "5 mi", "20 °c to f", "3,5 gb in mb". The result
//! uses the calculator row; Enter copies the converted value.
//!
//! Without a target, every unit has a usual counterpart (PS → kW, mi → km, °F → °C).
//! Data sizes follow Explorer: 1 KB = 1024 bytes (KiB, MiB … are the same); bits use
//! decimal prefixes like network speeds (1 Mbit = 1,000,000 bits).
//!
//! Short names that would take over a normal search (`d` for "3D Viewer", `k` for
//! "4K", `b`, `a`, `st`, `ct`, `pt`) are left out; their long names still work.

use crate::calc::{format_number, Answer};

/// Icon of the unit row ("Segoe MDL2 Assets": Ruler).
const GLYPH: char = '\u{ED5E}';

/// Words between the value and the target unit ("in" is also inches, see `parse`).
const CONNECTORS: &[&str] = &["in", "to", "into", "nach", "zu", "as", "=", "->"];

/// Significant digits of a converted value (more would only show float noise).
const DIGITS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dim {
    Length,
    Area,
    Volume,
    Mass,
    Temperature,
    Speed,
    Power,
    Energy,
    Pressure,
    Data,
    Time,
}

#[derive(Debug, PartialEq)]
pub struct Unit {
    /// Shown symbol, also the key for `default`.
    id: &'static str,
    /// German symbol, if different.
    de: Option<&'static str>,
    dim: Dim,
    /// Value in the base unit = value × factor + offset (offset only for temperatures).
    factor: f64,
    offset: f64,
    /// What people type (lowercase).
    names: &'static [&'static str],
    /// Target when none is typed (an `id` of the same dimension).
    default: &'static str,
}

const fn unit(
    id: &'static str,
    dim: Dim,
    factor: f64,
    names: &'static [&'static str],
    default: &'static str,
) -> Unit {
    Unit {
        id,
        de: None,
        dim,
        factor,
        offset: 0.0,
        names,
        default,
    }
}

const KIB: f64 = 1024.0;
const FAHRENHEIT: f64 = 5.0 / 9.0;

/// Base units: metre, square metre, litre, kilogram, kelvin, m/s, watt, joule, pascal,
/// byte, second.
static UNITS: &[Unit] = &[
    // Length
    unit(
        "mm",
        Dim::Length,
        0.001,
        &[
            "mm",
            "millimeter",
            "millimetre",
            "millimeters",
            "millimetres",
        ],
        "in",
    ),
    unit(
        "cm",
        Dim::Length,
        0.01,
        &[
            "cm",
            "zentimeter",
            "centimeter",
            "centimetre",
            "centimeters",
            "centimetres",
        ],
        "in",
    ),
    unit(
        "dm",
        Dim::Length,
        0.1,
        &["dm", "dezimeter", "decimeter"],
        "cm",
    ),
    unit(
        "m",
        Dim::Length,
        1.0,
        &["m", "meter", "metre", "meters", "metres"],
        "ft",
    ),
    unit(
        "km",
        Dim::Length,
        1000.0,
        &["km", "kilometer", "kilometre", "kilometers", "kilometres"],
        "mi",
    ),
    unit(
        "in",
        Dim::Length,
        0.0254,
        &["in", "inch", "inches", "zoll"],
        "cm",
    ),
    unit(
        "ft",
        Dim::Length,
        0.3048,
        &["ft", "foot", "feet", "fuß", "fuss"],
        "m",
    ),
    unit("yd", Dim::Length, 0.9144, &["yd", "yard", "yards"], "m"),
    unit(
        "mi",
        Dim::Length,
        1609.344,
        &["mi", "mile", "miles", "meile", "meilen"],
        "km",
    ),
    Unit {
        de: Some("sm"),
        ..unit(
            "nmi",
            Dim::Length,
            1852.0,
            &["nmi", "sm", "seemeile", "seemeilen", "nautical"],
            "km",
        )
    },
    // Area
    unit("mm²", Dim::Area, 1e-6, &["mm²", "mm2", "qmm"], "cm²"),
    unit("cm²", Dim::Area, 1e-4, &["cm²", "cm2", "qcm"], "in²"),
    unit("m²", Dim::Area, 1.0, &["m²", "m2", "qm"], "ft²"),
    unit(
        "ha",
        Dim::Area,
        1e4,
        &["ha", "hektar", "hectare", "hectares"],
        "acre",
    ),
    unit("km²", Dim::Area, 1e6, &["km²", "km2", "qkm"], "mi²"),
    unit("in²", Dim::Area, 0.00064516, &["in²", "in2", "sqin"], "cm²"),
    unit("ft²", Dim::Area, 0.09290304, &["ft²", "ft2", "sqft"], "m²"),
    unit(
        "acre",
        Dim::Area,
        4046.8564224,
        &["acre", "acres", "ac"],
        "ha",
    ),
    unit(
        "mi²",
        Dim::Area,
        2589988.110336,
        &["mi²", "mi2", "sqmi"],
        "km²",
    ),
    // Volume
    unit(
        "ml",
        Dim::Volume,
        0.001,
        &[
            "ml",
            "milliliter",
            "millilitre",
            "milliliters",
            "millilitres",
        ],
        "fl oz",
    ),
    unit(
        "cl",
        Dim::Volume,
        0.01,
        &["cl", "zentiliter", "centiliter", "centilitre"],
        "ml",
    ),
    unit(
        "dl",
        Dim::Volume,
        0.1,
        &["dl", "deziliter", "deciliter", "decilitre"],
        "ml",
    ),
    unit(
        "l",
        Dim::Volume,
        1.0,
        &["l", "liter", "litre", "liters", "litres"],
        "gal",
    ),
    unit(
        "hl",
        Dim::Volume,
        100.0,
        &["hl", "hektoliter", "hectoliter", "hectolitre"],
        "l",
    ),
    unit(
        "cm³",
        Dim::Volume,
        0.001,
        &["cm³", "cm3", "ccm", "cc"],
        "ml",
    ),
    unit(
        "m³",
        Dim::Volume,
        1000.0,
        &["m³", "m3", "cbm", "kubikmeter"],
        "l",
    ),
    unit("fl oz", Dim::Volume, 0.0295735295625, &["floz"], "ml"),
    unit("pint", Dim::Volume, 0.473176473, &["pint", "pints"], "l"),
    unit(
        "gal",
        Dim::Volume,
        3.785411784,
        &["gal", "gallon", "gallons", "gallone", "gallonen"],
        "l",
    ),
    // Mass
    unit(
        "mg",
        Dim::Mass,
        1e-6,
        &["mg", "milligramm", "milligram", "milligrams"],
        "g",
    ),
    unit(
        "g",
        Dim::Mass,
        0.001,
        &["g", "gramm", "gram", "grams", "gr"],
        "oz",
    ),
    unit(
        "kg",
        Dim::Mass,
        1.0,
        &["kg", "kilo", "kilos", "kilogramm", "kilogram", "kilograms"],
        "lb",
    ),
    unit(
        "t",
        Dim::Mass,
        1000.0,
        &["t", "tonne", "tonnen", "tonnes"],
        "kg",
    ),
    unit("ct", Dim::Mass, 0.0002, &["karat", "carat", "carats"], "g"),
    unit(
        "oz",
        Dim::Mass,
        0.028349523125,
        &["oz", "ounce", "ounces", "unze", "unzen"],
        "g",
    ),
    unit("lb", Dim::Mass, 0.45359237, &["lb", "lbs"], "kg"),
    unit("st", Dim::Mass, 6.35029318, &["stone", "stones"], "kg"),
    // Temperature (base: kelvin)
    Unit {
        offset: 273.15,
        ..unit(
            "°C",
            Dim::Temperature,
            1.0,
            &["°c", "c", "celsius", "grad"],
            "°F",
        )
    },
    Unit {
        offset: 273.15 - 32.0 * FAHRENHEIT,
        ..unit(
            "°F",
            Dim::Temperature,
            FAHRENHEIT,
            &["°f", "f", "fahrenheit"],
            "°C",
        )
    },
    unit("K", Dim::Temperature, 1.0, &["kelvin"], "°C"),
    // Speed
    unit("m/s", Dim::Speed, 1.0, &["m/s"], "km/h"),
    unit(
        "km/h",
        Dim::Speed,
        1.0 / 3.6,
        &["km/h", "kmh", "kph"],
        "mph",
    ),
    unit("mph", Dim::Speed, 0.44704, &["mph"], "km/h"),
    unit(
        "kn",
        Dim::Speed,
        1852.0 / 3600.0,
        &["kn", "knoten", "knot", "knots"],
        "km/h",
    ),
    // Power
    unit("W", Dim::Power, 1.0, &["w", "watt"], "kW"),
    unit("kW", Dim::Power, 1000.0, &["kw", "kilowatt"], "PS"),
    unit("MW", Dim::Power, 1e6, &["mw", "megawatt"], "kW"),
    unit(
        "PS",
        Dim::Power,
        735.49875,
        &["ps", "pferdestärke", "pferdestärken"],
        "kW",
    ),
    unit(
        "hp",
        Dim::Power,
        745.699_871_582_270_2,
        &["hp", "horsepower", "bhp"],
        "kW",
    ),
    // Energy
    unit("J", Dim::Energy, 1.0, &["j", "joule"], "cal"),
    unit("kJ", Dim::Energy, 1000.0, &["kj", "kilojoule"], "kcal"),
    unit("MJ", Dim::Energy, 1e6, &["mj", "megajoule"], "kWh"),
    unit(
        "cal",
        Dim::Energy,
        4.184,
        &["cal", "kalorie", "kalorien", "calorie", "calories"],
        "J",
    ),
    unit(
        "kcal",
        Dim::Energy,
        4184.0,
        &["kcal", "kilokalorie", "kilokalorien"],
        "kJ",
    ),
    unit("Wh", Dim::Energy, 3600.0, &["wh"], "J"),
    unit("kWh", Dim::Energy, 3.6e6, &["kwh"], "MJ"),
    unit("MWh", Dim::Energy, 3.6e9, &["mwh"], "kWh"),
    unit("BTU", Dim::Energy, 1055.05585262, &["btu"], "kWh"),
    // Pressure
    unit("Pa", Dim::Pressure, 1.0, &["pa", "pascal"], "hPa"),
    unit("hPa", Dim::Pressure, 100.0, &["hpa"], "bar"),
    unit("kPa", Dim::Pressure, 1000.0, &["kpa"], "psi"),
    unit("MPa", Dim::Pressure, 1e6, &["mpa"], "bar"),
    unit("mbar", Dim::Pressure, 100.0, &["mbar", "millibar"], "bar"),
    unit("bar", Dim::Pressure, 1e5, &["bar"], "psi"),
    unit("psi", Dim::Pressure, 6894.757293168, &["psi"], "bar"),
    unit("atm", Dim::Pressure, 101325.0, &["atm"], "bar"),
    unit("mmHg", Dim::Pressure, 133.322387415, &["mmhg"], "hPa"),
    unit("Torr", Dim::Pressure, 101325.0 / 760.0, &["torr"], "hPa"),
    // Data (base: byte; KB = 1024 like Explorer)
    unit("bit", Dim::Data, 0.125, &["bit", "bits"], "Byte"),
    unit("kbit", Dim::Data, 125.0, &["kbit"], "KB"),
    unit("Mbit", Dim::Data, 125_000.0, &["mbit"], "MB"),
    unit("Gbit", Dim::Data, 125_000_000.0, &["gbit"], "GB"),
    unit("Byte", Dim::Data, 1.0, &["byte", "bytes"], "KB"),
    unit(
        "KB",
        Dim::Data,
        KIB,
        &["kb", "kib", "kilobyte", "kilobytes"],
        "Byte",
    ),
    unit(
        "MB",
        Dim::Data,
        KIB * KIB,
        &["mb", "mib", "megabyte", "megabytes"],
        "KB",
    ),
    unit(
        "GB",
        Dim::Data,
        KIB * KIB * KIB,
        &["gb", "gib", "gigabyte", "gigabytes"],
        "MB",
    ),
    unit(
        "TB",
        Dim::Data,
        KIB * KIB * KIB * KIB,
        &["tb", "tib", "terabyte", "terabytes"],
        "GB",
    ),
    // Time
    unit(
        "ms",
        Dim::Time,
        0.001,
        &[
            "ms",
            "millisekunde",
            "millisekunden",
            "millisecond",
            "milliseconds",
        ],
        "s",
    ),
    unit(
        "s",
        Dim::Time,
        1.0,
        &[
            "s", "sek", "sec", "sekunde", "sekunden", "second", "seconds",
        ],
        "min",
    ),
    unit(
        "min",
        Dim::Time,
        60.0,
        &["min", "minute", "minuten", "minutes"],
        "h",
    ),
    unit(
        "h",
        Dim::Time,
        3600.0,
        &["h", "std", "stunde", "stunden", "hour", "hours"],
        "min",
    ),
    unit(
        "d",
        Dim::Time,
        86400.0,
        &["tag", "tage", "tagen", "day", "days"],
        "h",
    ),
    Unit {
        de: Some("Wo."),
        ..unit(
            "wk",
            Dim::Time,
            604_800.0,
            &["woche", "wochen", "week", "weeks"],
            "d",
        )
    },
    Unit {
        de: Some("Mon."),
        ..unit(
            "mo",
            Dim::Time,
            2_629_800.0,
            &["monat", "monate", "monaten", "month", "months"],
            "d",
        )
    },
    Unit {
        de: Some("J."),
        ..unit(
            "yr",
            Dim::Time,
            31_557_600.0,
            &["jahr", "jahre", "jahren", "year", "years"],
            "d",
        )
    },
];

fn by_name(word: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.names.contains(&word))
}

fn by_id(id: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.id == id)
}

impl Unit {
    fn symbol(&self, language: &str) -> &'static str {
        match (language, self.de) {
            ("de", Some(de)) => de,
            _ => self.id,
        }
    }
}

/// `value` of `from` in `to` (same dimension).
fn convert(value: f64, from: &Unit, to: &Unit) -> f64 {
    (value * from.factor + from.offset - to.offset) / to.factor
}

/// What was typed: value, source unit and, if given, the target unit.
#[derive(Debug, PartialEq)]
pub struct Conversion {
    pub value: f64,
    pub from: &'static Unit,
    pub to: Option<&'static Unit>,
}

#[derive(Debug, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
}

/// Characters a unit can continue with after its first letter ("km/h", "m²", "m2").
fn unit_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '²' | '³')
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.trim().to_lowercase().chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next_is_digit = |j: usize| chars.get(j).is_some_and(|d| d.is_ascii_digit());
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || ((c == '.' || c == ',') && next_is_digit(i + 1))
            || (c == '-' && tokens.is_empty() && next_is_digit(i + 1))
        {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | ',')) {
                i += 1;
            }
            let number: String = chars[start..i].iter().collect::<String>().replace(',', ".");
            tokens.push(Token::Number(number.parse().ok()?));
        } else if c.is_alphabetic() || c == '°' {
            let start = i;
            i += 1;
            while i < chars.len() && unit_char(chars[i]) {
                i += 1;
            }
            tokens.push(Token::Word(chars[start..i].iter().collect()));
        } else if c == '-' && chars.get(i + 1) == Some(&'>') {
            tokens.push(Token::Word("->".into()));
            i += 2;
        } else if c == '=' {
            tokens.push(Token::Word("=".into()));
            i += 1;
        } else {
            return None;
        }
    }
    // "fl oz", "sq ft" … are typed as two words.
    let mut merged: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let (Some(Token::Word(prev)), Token::Word(word)) = (merged.last_mut(), &token) {
            if matches!(prev.as_str(), "fl" | "sq") && by_name(&format!("{prev}{word}")).is_some() {
                prev.push_str(word);
                continue;
            }
        }
        merged.push(token);
    }
    Some(merged)
}

/// "100 ps", "100ps in kw", "100 ps kw", "-5 °c to °f", "5 fl oz in ml".
pub fn parse(text: &str) -> Option<Conversion> {
    let tokens = tokenize(text)?;
    let unit_of = |t: &Token| match t {
        Token::Word(w) => by_name(w),
        Token::Number(_) => None,
    };
    let [Token::Number(value), from, rest @ ..] = tokens.as_slice() else {
        return None;
    };
    let from = unit_of(from)?;
    let same_dim = |t: &Token| unit_of(t).filter(|u| u.dim == from.dim);
    let is_connector = |t: &Token| matches!(t, Token::Word(w) if CONNECTORS.contains(&w.as_str()));
    let to = match rest {
        [] => None,
        // "100 cm in" while typing: "in" is a connector here, not inches.
        [t] => match same_dim(t) {
            Some(u) => Some(u),
            None if is_connector(t) => None,
            None => return None,
        },
        [c, t] if is_connector(c) => Some(same_dim(t)?),
        _ => return None,
    };
    Some(Conversion {
        value: *value,
        from,
        to,
    })
}

/// `value` rounded to `DIGITS` significant digits.
fn round_significant(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let scale = 10f64.powi(DIGITS - 1 - value.abs().log10().floor() as i32);
    if !scale.is_finite() || scale == 0.0 {
        return value;
    }
    (value * scale).round() / scale
}

/// The converter row for `text`, if it is a unit conversion.
pub fn answer(text: &str, language: &str) -> Option<Answer> {
    let conversion = parse(text)?;
    let from = conversion.from;
    let to = conversion.to.or_else(|| by_id(from.default))?;
    if std::ptr::eq(from, to) {
        return None;
    }
    let result = round_significant(convert(conversion.value, from, to));
    if !result.is_finite() {
        return None;
    }
    let (decimal, group) = if language == "de" {
        (',', '.')
    } else {
        ('.', ',')
    };
    let value = format_number(conversion.value, decimal, Some(group));
    let converted = format_number(result, decimal, Some(group));
    Some(Answer {
        display: format!(
            "{value} {} = {converted} {}",
            from.symbol(language),
            to.symbol(language)
        ),
        copy: format_number(result, decimal, None),
        label: Some(crate::i18n::t("result-kind-units")),
        glyph: GLYPH,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    fn conv(value: f64, from: &str, to: &str) -> f64 {
        convert(value, by_id(from).unwrap(), by_id(to).unwrap())
    }

    #[test]
    fn table_is_consistent() {
        for u in UNITS {
            let default = by_id(u.default).unwrap_or_else(|| panic!("default of {}", u.id));
            assert_eq!(default.dim, u.dim, "default of {}", u.id);
            assert_ne!(default.id, u.id, "default of {}", u.id);
            for name in u.names {
                assert_eq!(*name, name.to_lowercase(), "{name}");
                let owners = UNITS.iter().filter(|o| o.names.contains(name)).count();
                assert_eq!(owners, 1, "name {name} used twice");
                assert_eq!(
                    tokenize(&format!("1 {name}")).map(|t| t.len()),
                    Some(2),
                    "{name}"
                );
            }
            assert_eq!(UNITS.iter().filter(|o| o.id == u.id).count(), 1, "{}", u.id);
        }
    }

    #[test]
    fn units_do_not_take_over_currencies_or_searches() {
        for word in [
            "pound", "pfund", "dollar", "rand", "d", "k", "b", "a", "st", "ct", "pt",
        ] {
            assert_eq!(by_name(word), None, "{word}");
        }
        for s in [
            "3d",
            "4k",
            "7 days to die",
            "Windows 11",
            "100",
            "kw",
            "1+2",
            "100 ps in kg",
            "5 m s",
            "100 usd",
        ] {
            assert_eq!(parse(s), None, "{s}");
        }
    }

    #[test]
    fn converts() {
        assert!(close(conv(100.0, "PS", "kW"), 73.549875));
        assert!(close(conv(1.0, "mi", "km"), 1.609344));
        assert!(close(conv(100.0, "°C", "°F"), 212.0));
        assert!(close(conv(-40.0, "°F", "°C"), -40.0));
        assert!(close(conv(0.0, "°C", "K"), 273.15));
        assert!(close(conv(1.0, "GB", "MB"), 1024.0));
        assert!(close(conv(100.0, "Mbit", "MB"), 100e6 / 8.0 / 1048576.0));
        assert!(close(conv(36.0, "km/h", "m/s"), 10.0));
        assert!(close(conv(1.0, "kWh", "MJ"), 3.6));
        assert!(close(conv(1.0, "bar", "hPa"), 1000.0));
        assert!(close(conv(1.0, "d", "h"), 24.0));
        assert!(close(conv(1.0, "m³", "l"), 1000.0));
        assert!(close(conv(1.0, "ha", "m²"), 10000.0));
    }

    #[test]
    fn understands_typical_input() {
        let p = |s: &str| parse(s).map(|c| (c.value, c.from.id, c.to.map(|u| u.id)));
        assert_eq!(p("100 ps"), Some((100.0, "PS", None)));
        assert_eq!(p("100PS in kW"), Some((100.0, "PS", Some("kW"))));
        assert_eq!(p("100 ps kw"), Some((100.0, "PS", Some("kW"))));
        assert_eq!(p("-5 °c to °f"), Some((-5.0, "°C", Some("°F"))));
        assert_eq!(p("-5 c nach f"), Some((-5.0, "°C", Some("°F"))));
        assert_eq!(p("12,5 km/h -> mph"), Some((12.5, "km/h", Some("mph"))));
        assert_eq!(p("20 m2 in sq ft"), Some((20.0, "m²", Some("ft²"))));
        assert_eq!(p("5 fl oz in ml"), Some((5.0, "fl oz", Some("ml"))));
        assert_eq!(p("3,5 gb = mb"), Some((3.5, "GB", Some("MB"))));
        assert_eq!(
            p("100 kg in"),
            Some((100.0, "kg", None)),
            "connector while typing"
        );
        assert_eq!(p("100 cm in"), Some((100.0, "cm", Some("in"))));
        assert_eq!(p("100 cm in in"), Some((100.0, "cm", Some("in"))));
        assert_eq!(p("2 stunden in min"), Some((2.0, "h", Some("min"))));
    }

    #[test]
    fn formats_answer() {
        let a = answer("100 ps", "de").unwrap();
        assert_eq!(a.display, "100 PS = 73,549875 kW");
        assert_eq!(a.copy, "73,549875");
        let a = answer("1 kg", "en").unwrap();
        assert_eq!(a.display, "1 kg = 2.2046226 lb");
        let a = answer("1 woche in tage", "de").unwrap();
        assert_eq!(a.display, "1 Wo. = 7 d");
        let a = answer("2000 mi", "en").unwrap();
        assert_eq!(a.display, "2,000 mi = 3,218.688 km");
        assert_eq!(answer("100 ps in ps", "en"), None);
    }
}
