//! Calculator: if the search text is an arithmetic expression, its result is shown as
//! the first row; Enter copies it.
//!
//! Supported: `+ - * / : ^`, `×` `÷` `·`, parentheses, decimal comma or point,
//! `%` (`85 + 20%` = 102, `20% von 85` = 17), the constants `pi` and `e`, and
//! `sqrt abs round floor ceil ln log sin cos tan` (radians).
//! A bare number is not a calculation, and anything else (an app name like "7-Zip")
//! simply fails to parse.

/// A computed result (calculator or currency converter), shown as the first row.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// Shown in the list, e.g. "= 1.234,5".
    pub display: String,
    /// Copied by Enter, e.g. "1234,5" (no digit grouping, so it pastes as a number).
    /// Empty: nothing to copy yet (e.g. currency rates still loading).
    pub copy: String,
    /// Grey label on the right; `None`: "Calculator".
    pub label: Option<String>,
    /// Icon: a glyph of "Segoe MDL2 Assets".
    pub glyph: char,
}

/// The result of `text` if it is a calculation, formatted for `language` ("de" uses
/// a decimal comma and dots between thousands).
pub fn answer(text: &str, language: &str) -> Option<Answer> {
    let value = evaluate(text)?;
    let decimal = if language == "de" { ',' } else { '.' };
    let group = if language == "de" { '.' } else { ',' };
    Some(Answer {
        display: format!("= {}", format_number(value, decimal, Some(group))),
        copy: format_number(value, decimal, None),
        label: None,
        glyph: '\u{E8EF}', // Calculator
    })
}

/// Longest calculation accepted, in tokens (numbers, operators, parentheses, names).
const MAX_TOKENS: usize = 200;

/// Value of an expression; `None` if it is not one (or only a number).
pub fn evaluate(text: &str) -> Option<f64> {
    let text = text
        .trim()
        .to_lowercase()
        .replace(" von ", " * ")
        .replace(" of ", " * ");
    let tokens = tokenize(&text)?;
    // The parser recurses per parenthesis and sign: a bound keeps pasted nonsense
    // ("((((((…" thousands of times) from overflowing the stack. No real calculation
    // comes close.
    if tokens.len() > MAX_TOKENS {
        return None;
    }
    // A calculation needs a number (so typing "e" or "pi" still finds Edge or a
    // Pi-hole app) and something to calculate: an operator, % or a function.
    let has_number = tokens.iter().any(|t| matches!(t, Token::Num(_)));
    let calculates = tokens.iter().enumerate().any(|(i, t)| match t {
        Token::Op(c) => i > 0 || *c != '-', // a leading minus alone is just a sign
        Token::Percent | Token::Name(_) => true,
        _ => false,
    });
    if !(has_number && calculates) {
        return None;
    }
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expr()?.value;
    (parser.pos == parser.tokens.len() && value.is_finite()).then_some(value)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f64),
    Op(char),
    Percent,
    Open,
    Close,
    Name(String),
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '0'..='9' | '.' | ',' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | ','))
                {
                    i += 1;
                }
                // Both "1,5" and "1.5" are a decimal; no thousands separators.
                let number: String = chars[start..i].iter().collect::<String>().replace(',', ".");
                tokens.push(Token::Num(number.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '^' => {
                tokens.push(Token::Op(c));
                i += 1;
            }
            '×' | '·' => {
                tokens.push(Token::Op('*'));
                i += 1;
            }
            '÷' | ':' => {
                tokens.push(Token::Op('/'));
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            _ if c.is_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_alphabetic() {
                    i += 1;
                }
                tokens.push(Token::Name(chars[start..i].iter().collect()));
            }
            _ => return None,
        }
    }
    Some(tokens)
}

/// A value, and whether it is a bare percentage ("20%"), which `+`/`-` apply to the
/// left side: 85 + 20% = 85 * 1.2.
#[derive(Clone, Copy)]
struct Value {
    value: f64,
    percent: bool,
}

impl Value {
    fn plain(value: f64) -> Self {
        Value {
            value,
            percent: false,
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    /// expr = term (("+" | "-") term)*
    fn expr(&mut self) -> Option<Value> {
        let mut left = self.term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let right = self.term()?;
            let amount = if right.percent {
                left.value * right.value
            } else {
                right.value
            };
            left = Value::plain(if op == '+' {
                left.value + amount
            } else {
                left.value - amount
            });
        }
        Some(left)
    }

    /// term = unary (("*" | "/") unary)*
    fn term(&mut self) -> Option<Value> {
        let mut left = self.unary()?;
        while let Some(Token::Op(op @ ('*' | '/'))) = self.peek().cloned() {
            self.pos += 1;
            let right = self.unary()?;
            left = Value::plain(if op == '*' {
                left.value * right.value
            } else {
                left.value / right.value
            });
        }
        Some(left)
    }

    /// power = postfix ("^" unary)?   (right-associative: 2^3^2 = 2^9; -2^2 = -4)
    fn power(&mut self) -> Option<Value> {
        let base = self.postfix()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.pos += 1;
            let exponent = self.unary()?;
            return Some(Value::plain(base.value.powf(exponent.value)));
        }
        Some(base)
    }

    /// unary = ("-" | "+") unary | power   (the sign binds weaker than "^")
    fn unary(&mut self) -> Option<Value> {
        match self.peek() {
            Some(Token::Op('-')) => {
                self.pos += 1;
                let v = self.unary()?;
                Some(Value {
                    value: -v.value,
                    ..v
                })
            }
            Some(Token::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    /// postfix = primary "%"?
    fn postfix(&mut self) -> Option<Value> {
        let v = self.primary()?;
        if let Some(Token::Percent) = self.peek() {
            self.pos += 1;
            return Some(Value {
                value: v.value / 100.0,
                percent: true,
            });
        }
        Some(v)
    }

    /// primary = number | "(" expr ")" | constant | function "(" expr ")"
    fn primary(&mut self) -> Option<Value> {
        match self.next()? {
            Token::Num(n) => Some(Value::plain(n)),
            Token::Open => {
                let v = self.expr()?;
                (self.next()? == Token::Close).then_some(Value::plain(v.value))
            }
            Token::Name(name) => match name.as_str() {
                "pi" | "π" => Some(Value::plain(std::f64::consts::PI)),
                "e" => Some(Value::plain(std::f64::consts::E)),
                _ => {
                    let f: fn(f64) -> f64 = match name.as_str() {
                        "sqrt" | "wurzel" => f64::sqrt,
                        "abs" => f64::abs,
                        "round" => f64::round,
                        "floor" => f64::floor,
                        "ceil" => f64::ceil,
                        "ln" => f64::ln,
                        "log" => f64::log10,
                        "sin" => f64::sin,
                        "cos" => f64::cos,
                        "tan" => f64::tan,
                        _ => return None,
                    };
                    if self.next()? != Token::Open {
                        return None;
                    }
                    let v = self.expr()?;
                    (self.next()? == Token::Close).then_some(Value::plain(f(v.value)))
                }
            },
            _ => None,
        }
    }
}

/// Up to 12 significant digits (so 0.1 + 0.2 shows 0.3), trailing zeros dropped;
/// scientific notation for very large or small values.
pub fn format_number(value: f64, decimal: char, group: Option<char>) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs().log10().floor() as i32;
    if !(-6..15).contains(&magnitude) {
        let s = format!("{value:.6e}"); // e.g. 1.234560e20
        let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = trim_zeros(mantissa);
        return format!("{}e{exponent}", mantissa.replace('.', &decimal.to_string()));
    }
    let decimals = (11 - magnitude).clamp(0, 15) as usize;
    let fixed = trim_zeros(&format!("{value:.decimals$}"));
    let (sign, digits) = fixed
        .strip_prefix('-')
        .map_or(("", fixed.as_str()), |d| ("-", d));
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let int = match group {
        Some(g) => group_thousands(int, g),
        None => int.to_string(),
    };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}{decimal}{frac}")
    }
}

/// Exactly `decimals` digits after the separator (amounts of money: "1.234,50").
pub fn format_fixed(value: f64, decimals: usize, decimal: char, group: Option<char>) -> String {
    let fixed = format!("{value:.decimals$}");
    let (sign, digits) = fixed
        .strip_prefix('-')
        .map_or(("", fixed.as_str()), |d| ("-", d));
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let int = match group {
        Some(g) => group_thousands(int, g),
        None => int.to_string(),
    };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}{decimal}{frac}")
    }
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

fn group_thousands(int: &str, sep: char) -> String {
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_input_is_rejected_not_a_crash() {
        let deep = format!("{}1+1{}", "(".repeat(5000), ")".repeat(5000));
        assert_eq!(evaluate(&deep), None);
        assert_eq!(evaluate(&format!("{}1+1", "-".repeat(30000))), None);
        assert_eq!(evaluate("((1+2)*3)^2"), Some(81.0));
    }

    fn eval(s: &str) -> f64 {
        evaluate(s).unwrap_or_else(|| panic!("{s} should evaluate"))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1+2*3"), 7.0);
        assert_eq!(eval("(1+2)*3"), 9.0);
        assert_eq!(eval("2^3^2"), 512.0);
        assert_eq!(eval("-2^2"), -4.0);
        assert_eq!(eval("10:4"), 2.5);
        assert_eq!(eval("3 × 4 ÷ 2"), 6.0);
        assert_eq!(eval("1,5*2"), 3.0);
        assert_eq!(eval("1.5*2"), 3.0);
    }

    #[test]
    fn percent() {
        assert_eq!(eval("85 + 20%"), 102.0);
        assert_eq!(eval("100 - 10%"), 90.0);
        assert_eq!(eval("20% von 85"), 17.0);
        assert_eq!(eval("50*10%"), 5.0);
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(eval("sqrt(16)"), 4.0);
        assert_eq!(eval("round(2,5)+1"), 4.0);
        assert!((eval("2*pi") - std::f64::consts::TAU).abs() < 1e-12);
    }

    #[test]
    fn not_a_calculation() {
        for s in [
            "",
            "42",
            "-5",
            "e",
            "pi",
            "ln",
            "e-mail",
            "Windows 11",
            "7-Zip",
            "1Password",
            "3DMark",
            "chrome",
            "1+",
            "(1+2",
            "1/0",
            "foo(2)",
        ] {
            assert_eq!(evaluate(s), None, "{s}");
        }
    }

    #[test]
    fn formatting() {
        assert_eq!(format_number(0.1 + 0.2, ',', Some('.')), "0,3");
        assert_eq!(format_number(1234567.5, ',', Some('.')), "1.234.567,5");
        assert_eq!(format_number(1234567.5, ',', None), "1234567,5");
        assert_eq!(format_number(-1234.0, '.', Some(',')), "-1,234");
        assert_eq!(format_number(1.0 / 3.0, '.', None), "0.333333333333");
        assert_eq!(format_number(2e20, '.', None), "2e20");
        assert_eq!(format_number(1.5e-9, ',', None), "1,5e-9");
        assert_eq!(format_fixed(1234.5, 2, ',', Some('.')), "1.234,50");
        assert_eq!(format_fixed(0.004, 2, '.', None), "0.00");
    }

    #[test]
    fn answer_is_localized() {
        let a = answer("1000*1,5", "de").unwrap();
        assert_eq!(a.display, "= 1.500");
        assert_eq!(a.copy, "1500");
        assert_eq!(answer("1/4", "en").unwrap().display, "= 0.25");
    }
}
