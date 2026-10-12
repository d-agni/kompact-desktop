//! Spotlight's calculator (6m-a): `2+3*4`, `(1.5+2)^2`, `10/4`, `-3 × 7`, `9 ÷ 3`. Pure and
//! tested; the page calls it through the `calculate` command as you type.

/// The result of `text` as Spotlight shows it, or `None` if `text` isn't a calculation: it must
/// parse completely, contain at least one operator (a lone number isn't a sum), and come out
/// finite.
pub fn calculate(text: &str) -> Option<String> {
    let tokens = tokenize(text)?;
    if !tokens.iter().any(|t| matches!(t, Token::Op(_))) {
        return None;
    }
    let mut p = Parser { tokens: &tokens, pos: 0 };
    let value = p.expr()?;
    if p.pos != tokens.len() || !value.is_finite() {
        return None;
    }
    Some(format(value))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Num(f64),
    Op(char),
    Open,
    Close,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '0'..='9' | '.' | ',' => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    match d {
                        '0'..='9' | '.' => s.push(d),
                        // Thousands separators: `1,000` is 1000.
                        ',' => {}
                        _ => break,
                    }
                    chars.next();
                }
                tokens.push(Token::Num(s.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '^' => {
                tokens.push(Token::Op(c));
                chars.next();
            }
            '×' | 'x' | 'X' => {
                tokens.push(Token::Op('*'));
                chars.next();
            }
            '÷' => {
                tokens.push(Token::Op('/'));
                chars.next();
            }
            '(' => {
                tokens.push(Token::Open);
                chars.next();
            }
            ')' => {
                tokens.push(Token::Close);
                chars.next();
            }
            _ => return None,
        }
    }
    (!tokens.is_empty()).then_some(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    fn eat_op(&mut self, ops: &[char]) -> Option<char> {
        match self.peek() {
            Some(Token::Op(o)) if ops.contains(&o) => {
                self.pos += 1;
                Some(o)
            }
            _ => None,
        }
    }

    /// expr = term (('+' | '-') term)*
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op) = self.eat_op(&['+', '-']) {
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    /// term = unary (('*' | '/') unary)*
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        while let Some(op) = self.eat_op(&['*', '/']) {
            let r = self.unary()?;
            v = if op == '*' { v * r } else { v / r };
        }
        Some(v)
    }

    /// unary = ('-' | '+') unary | power
    fn unary(&mut self) -> Option<f64> {
        match self.eat_op(&['-', '+']) {
            Some('-') => Some(-self.unary()?),
            Some(_) => self.unary(),
            None => self.power(),
        }
    }

    /// power = primary ('^' unary)?   (right-associative: 2^3^2 = 2^9)
    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.eat_op(&['^']).is_some() {
            return Some(base.powf(self.unary()?));
        }
        Some(base)
    }

    /// primary = number | '(' expr ')'
    fn primary(&mut self) -> Option<f64> {
        match self.peek()? {
            Token::Num(n) => {
                self.pos += 1;
                Some(n)
            }
            Token::Open => {
                self.pos += 1;
                let v = self.expr()?;
                (self.peek()? == Token::Close).then(|| self.pos += 1)?;
                Some(v)
            }
            _ => None,
        }
    }
}

/// Up to 10 significant digits, no trailing zeros, no `-0`; very large or small numbers in
/// e-notation.
fn format(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let mag = v.abs().log10().floor() as i32;
    if !(-6..15).contains(&mag) {
        return format!("{v:.6e}");
    }
    let decimals = (9 - mag).clamp(0, 15) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Option<String> {
        calculate(s)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(c("2+3*4").as_deref(), Some("14"));
        assert_eq!(c("(2+3)*4").as_deref(), Some("20"));
        assert_eq!(c("10/4").as_deref(), Some("2.5"));
        assert_eq!(c("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(c("2^10").as_deref(), Some("1024"));
        assert_eq!(c("2^3^2").as_deref(), Some("512"));
        assert_eq!(c("-3 × 7").as_deref(), Some("-21"));
        assert_eq!(c("9 ÷ 3").as_deref(), Some("3"));
        assert_eq!(c("1,000 * 3").as_deref(), Some("3000"));
        assert_eq!(c("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(c("-2^2").as_deref(), Some("-4"));
        assert_eq!(c("1-1").as_deref(), Some("0"));
    }

    #[test]
    fn not_calculations() {
        for s in ["", "42", "firefox", "2+", "(2+3", "2+3)", "1/0", "2 3", "x", "text editor", "..+1"] {
            assert_eq!(c(s), None, "{s:?}");
        }
    }

    #[test]
    fn big_and_small() {
        assert_eq!(c("10^20").as_deref(), Some("1.000000e20"));
        assert_eq!(c("1/10^7").as_deref(), Some("1.000000e-7"));
        assert_eq!(c("123456789*1000").as_deref(), Some("123456789000"));
    }
}
