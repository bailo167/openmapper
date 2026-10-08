// SPDX-License-Identifier: Apache-2.0
//! Pass-size expressions such as `"$WIDTH/2"` or `"floor($HEIGHT*scale)"`.
//!
//! Grammar: numbers, `$WIDTH`, `$HEIGHT`, `$name` / `name` for float/long
//! inputs, `+ - * /`, unary minus, parentheses and `floor ceil round min max
//! abs`.

use std::collections::HashMap;

use crate::IsfError;

/// Evaluates a size expression and rounds to a pixel count of at least 1.
pub fn eval_dimension(
    expr: &str,
    width: f64,
    height: f64,
    vars: &HashMap<String, f64>,
) -> Result<u32, IsfError> {
    let err = |m: &str| IsfError::Expression {
        expr: expr.to_owned(),
        message: m.to_owned(),
    };
    if expr.len() > MAX_EXPR_BYTES {
        return Err(err("expression is too long"));
    }
    let mut p = Parser {
        s: expr.as_bytes(),
        i: 0,
        depth: 0,
        width,
        height,
        vars,
    };
    let v = p.sum().map_err(|m| err(&m))?;
    p.skip_ws();
    if p.i != p.s.len() {
        return Err(err("unexpected trailing characters"));
    }
    if !v.is_finite() {
        return Err(err("result is not finite"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(v.round().clamp(1.0, 16384.0) as u32)
}

/// Longest size expression accepted.
pub const MAX_EXPR_BYTES: usize = 256;
/// Deepest nesting of parentheses, calls and unary minus (the parser is
/// recursive; untrusted shaders must not overflow the stack).
pub const MAX_DEPTH: usize = 32;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
    width: f64,
    height: f64,
    vars: &'a HashMap<String, f64>,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Result<f64, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("expression is nested too deeply".into());
        }
        let v = self.sum_inner();
        self.depth -= 1;
        v
    }

    fn sum_inner(&mut self) -> Result<f64, String> {
        let mut v = self.product()?;
        loop {
            if self.eat(b'+') {
                v += self.product()?;
            } else if self.eat(b'-') {
                v -= self.product()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            if self.eat(b'*') {
                v *= self.unary()?;
            } else if self.eat(b'/') {
                v /= self.unary()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        let mut negate = false;
        while self.eat(b'-') {
            negate = !negate;
        }
        let v = self.atom()?;
        Ok(if negate { -v } else { v })
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn atom(&mut self) -> Result<f64, String> {
        self.skip_ws();
        if self.eat(b'(') {
            let v = self.sum()?;
            if !self.eat(b')') {
                return Err("missing ')'".into());
            }
            return Ok(v);
        }
        let c = *self.s.get(self.i).ok_or("unexpected end")?;
        if c.is_ascii_digit() || c == b'.' {
            let start = self.i;
            while self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'.' || *c == b'e' || *c == b'E')
            {
                self.i += 1;
            }
            return std::str::from_utf8(&self.s[start..self.i])
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| "bad number".into());
        }
        let dollar = self.eat(b'$');
        let name = self.ident();
        if name.is_empty() {
            return Err(format!("unexpected character {:?}", c as char));
        }
        match name.as_str() {
            "WIDTH" if dollar => return Ok(self.width),
            "HEIGHT" if dollar => return Ok(self.height),
            _ => {}
        }
        if self.eat(b'(') {
            let a = self.sum()?;
            let b = if self.eat(b',') {
                Some(self.sum()?)
            } else {
                None
            };
            if !self.eat(b')') {
                return Err("missing ')'".into());
            }
            return match (name.as_str(), b) {
                ("floor", None) => Ok(a.floor()),
                ("ceil", None) => Ok(a.ceil()),
                ("round", None) => Ok(a.round()),
                ("abs", None) => Ok(a.abs()),
                ("min", Some(b)) => Ok(a.min(b)),
                ("max", Some(b)) => Ok(a.max(b)),
                _ => Err(format!("unknown function {name}")),
            };
        }
        self.vars
            .get(&name)
            .copied()
            .ok_or_else(|| format!("unknown variable {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_size_expressions() {
        let vars = HashMap::from([("scale".to_owned(), 0.25)]);
        let e = |s: &str| eval_dimension(s, 1920.0, 1080.0, &vars);
        assert_eq!(e("$WIDTH").unwrap(), 1920);
        assert_eq!(e("$WIDTH/2").unwrap(), 960);
        assert_eq!(e("floor($HEIGHT * scale) + 1").unwrap(), 271);
        assert_eq!(e("max($WIDTH, $HEIGHT) / -(-4)").unwrap(), 480);
        assert_eq!(e("1").unwrap(), 1);
        assert_eq!(e("0").unwrap(), 1, "at least one pixel");
        assert!(e("$WIDTH +").is_err());
        assert!(e("nope * 2").is_err());
        assert!(e("1/0").is_err());
        assert_eq!(e("---2").unwrap(), 1, "unary minus chains (−2 → 1 px)");
    }

    #[test]
    fn hostile_nesting_and_length_are_errors_not_stack_overflows() {
        let vars = HashMap::new();
        let e = |s: &str| eval_dimension(s, 64.0, 64.0, &vars);
        let nested = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
        assert_eq!(e(&nested(20)).unwrap(), 1);
        assert!(e(&nested(100)).is_err(), "too deep");
        let calls = format!("{}1{}", "floor(".repeat(40), ")".repeat(40));
        assert!(e(&calls).is_err());
        assert!(e(&"(".repeat(100_000)).is_err(), "too long");
        assert_eq!(e(&format!("{}2", "-".repeat(200))).unwrap(), 2);
    }
}
