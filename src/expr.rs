//! Parameter expressions — Houdini's channel references, with arithmetic.
//!
//! A parameter whose `expr` flag is set holds an EXPRESSION rather than a
//! value, and is evaluated every time the node is: `ch("../sphere1/Radius")
//! * 2 + 1`, `$F / 24`, `chs("../text1/Font")`. The flag is the model, not a
//! guess about the text: a kernel's Code contains `chf(`, a node name is an
//! identifier and `0.5` is an expression too, so anything that decided by
//! looking at the string would be wrong somewhere. Houdini makes the same
//! choice — a parm has an expression or it has a value.
//!
//! The language is deliberately small. Numbers and strings; `+ - * / % ^`,
//! comparisons, `&& || !`; a fixed set of functions; the `$F` / `$FF` frame
//! variables; and the channel functions, which are the whole point:
//! `ch(path)` reads a parameter as a number (a toggle 1 or 0, a choice its
//! option index, a float3 component through `.x` / `.y` / `.z`), `chs` as a
//! string, `chf` / `chi` / `chb` are `ch` with the kernel vocabulary's
//! conversions. Paths are Houdini's: relative to the node that holds the
//! expression, `..` its parent, a leading `/` the root, and a bare name the
//! node's OWN parameter. No ternary, because `:` separates a float3's
//! components; `if(cond, a, b)` is the function instead.
//!
//! This module knows nothing about nodes: [`Scope`] is how an evaluation
//! reaches a channel, and `geometry.rs` implements it over the tree.

use std::fmt::Write as _;

/// A parameter's evaluated value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
}

impl Value {
    pub fn as_num(&self) -> f64 {
        match self {
            Value::Num(n) => *n,
            Value::Str(s) => {
                let t = s.trim();
                if t.eq_ignore_ascii_case("true") {
                    1.0
                } else if t.eq_ignore_ascii_case("false") {
                    0.0
                } else {
                    t.parse::<f64>().unwrap_or(0.0)
                }
            }
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Value::Num(n) => fmt_num(*n),
            Value::Str(s) => s.clone(),
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty() && !s.eq_ignore_ascii_case("false") && s != "0",
        }
    }
}

/// A number as a parameter string: an integer when it is one, else the f32
/// it will be read back as — so `0.1 + 0.2` shows as `0.3`, not the f64
/// noise, and `2` is `2` rather than `2.0`.
pub fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v as f32)
    }
}

/// How a channel function wants the parameter it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChKind {
    /// `ch` / `chf`: a number.
    Float,
    /// `chi`: a number, truncated.
    Int,
    /// `chb`: 1 or 0.
    Bool,
    /// `chs`: the string as it is (a choice's option text, a toggle's
    /// `true` / `false`).
    Str,
}

/// What an evaluation asks of its surroundings.
pub trait Scope {
    /// The parameter at `path`, relative to the node holding the expression.
    fn channel(&mut self, path: &str, kind: ChKind) -> Result<Value, String>;
    /// A `$NAME` variable, or None when there is no such variable.
    fn var(&self, name: &str) -> Option<Value>;
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    Str(String),
    Var(String),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
    Ch(ChKind, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr(Node);

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Var(String),
    Sym(&'static str),
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text.parse::<f64>().map_err(|_| format!("bad number `{text}`"))?;
            out.push(Tok::Num(n));
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            loop {
                let Some(&d) = chars.get(i) else { return Err("unterminated string".to_string()) };
                i += 1;
                if d == quote {
                    break;
                }
                if d == '\\' {
                    if let Some(&e) = chars.get(i) {
                        s.push(e);
                        i += 1;
                    }
                    continue;
                }
                s.push(d);
            }
            out.push(Tok::Str(s));
            continue;
        }
        if c == '$' {
            let start = i + 1;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i == start {
                return Err("`$` with no variable name".to_string());
            }
            out.push(Tok::Var(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let sym = match two.as_str() {
            "<=" => Some("<="),
            ">=" => Some(">="),
            "==" => Some("=="),
            "!=" => Some("!="),
            "&&" => Some("&&"),
            "||" => Some("||"),
            _ => None,
        };
        if let Some(s) = sym {
            out.push(Tok::Sym(s));
            i += 2;
            continue;
        }
        let one = match c {
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '%' => "%",
            '^' => "^",
            '(' => "(",
            ')' => ")",
            ',' => ",",
            '<' => "<",
            '>' => ">",
            '!' => "!",
            _ => return Err(format!("unexpected `{c}`")),
        };
        out.push(Tok::Sym(one));
        i += 1;
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(t)) if *t == s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &str) -> Result<(), String> {
        if self.eat_sym(s) { Ok(()) } else { Err(format!("expected `{s}`")) }
    }

    fn or(&mut self) -> Result<Node, String> {
        let mut l = self.and()?;
        while self.eat_sym("||") {
            let r = self.and()?;
            l = Node::Bin(Op::Or, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Node, String> {
        let mut l = self.eq()?;
        while self.eat_sym("&&") {
            let r = self.eq()?;
            l = Node::Bin(Op::And, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn eq(&mut self) -> Result<Node, String> {
        let mut l = self.cmp()?;
        loop {
            let op = if self.eat_sym("==") { Op::Eq } else if self.eat_sym("!=") { Op::Ne } else { break };
            let r = self.cmp()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn cmp(&mut self) -> Result<Node, String> {
        let mut l = self.add()?;
        loop {
            let op = if self.eat_sym("<=") {
                Op::Le
            } else if self.eat_sym(">=") {
                Op::Ge
            } else if self.eat_sym("<") {
                Op::Lt
            } else if self.eat_sym(">") {
                Op::Gt
            } else {
                break;
            };
            let r = self.add()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn add(&mut self) -> Result<Node, String> {
        let mut l = self.mul()?;
        loop {
            let op = if self.eat_sym("+") { Op::Add } else if self.eat_sym("-") { Op::Sub } else { break };
            let r = self.mul()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn mul(&mut self) -> Result<Node, String> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat_sym("*") {
                Op::Mul
            } else if self.eat_sym("/") {
                Op::Div
            } else if self.eat_sym("%") {
                Op::Rem
            } else {
                break;
            };
            let r = self.unary()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Node, String> {
        if self.eat_sym("-") {
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        if self.eat_sym("!") {
            return Ok(Node::Not(Box::new(self.unary()?)));
        }
        if self.eat_sym("+") {
            return self.unary();
        }
        self.pow()
    }
    fn pow(&mut self) -> Result<Node, String> {
        let base = self.atom()?;
        if self.eat_sym("^") {
            // Right-associative: 2^3^2 is 2^9.
            let exp = self.unary()?;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }
    fn atom(&mut self) -> Result<Node, String> {
        let tok = self.peek().cloned().ok_or_else(|| "unexpected end of expression".to_string())?;
        self.pos += 1;
        match tok {
            Tok::Num(n) => Ok(Node::Num(n)),
            Tok::Str(s) => Ok(Node::Str(s)),
            Tok::Var(v) => Ok(Node::Var(v)),
            Tok::Sym("(") => {
                let inner = self.or()?;
                self.expect_sym(")")?;
                Ok(inner)
            }
            Tok::Ident(name) => {
                if self.eat_sym("(") {
                    let mut args = Vec::new();
                    if !self.eat_sym(")") {
                        loop {
                            args.push(self.or()?);
                            if self.eat_sym(",") {
                                continue;
                            }
                            self.expect_sym(")")?;
                            break;
                        }
                    }
                    let kind = match name.as_str() {
                        "ch" | "chf" => Some(ChKind::Float),
                        "chi" => Some(ChKind::Int),
                        "chb" => Some(ChKind::Bool),
                        "chs" => Some(ChKind::Str),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        return match args.as_slice() {
                            [Node::Str(path)] if !path.trim().is_empty() => Ok(Node::Ch(kind, path.trim().to_string())),
                            _ => Err(format!("{name}() takes one quoted path")),
                        };
                    }
                    Ok(Node::Call(name, args))
                } else {
                    match name.as_str() {
                        "PI" => Ok(Node::Num(std::f64::consts::PI)),
                        "E" => Ok(Node::Num(std::f64::consts::E)),
                        _ => Err(format!("unknown name `{name}`")),
                    }
                }
            }
            Tok::Sym(s) => Err(format!("unexpected `{s}`")),
        }
    }
}

/// Parse an expression. Errors name what went wrong, since they land on a
/// node's error slot for the user to read.
pub fn parse(src: &str) -> Result<Expr, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("empty expression".to_string());
    }
    let mut p = Parser { toks, pos: 0 };
    let node = p.or()?;
    if p.pos != p.toks.len() {
        return Err("trailing input after the expression".to_string());
    }
    Ok(Expr(node))
}

/// Whether `s` reads as an expression that REFERS to something — a channel
/// or a variable. This is the inference applied to a value typed or scripted
/// into a parameter that has no expression yet: `ch("../a/Radius")` becomes
/// one, while `1+2`, a node name and a kernel do not — arithmetic on a
/// literal is asked for through the row's Edit Expression, not guessed.
pub fn looks_like_expression(s: &str) -> bool {
    let t = s.trim();
    if t.len() > 512 || !(t.contains("ch") || t.contains('$')) {
        return false;
    }
    if let Ok(e) = parse(t) {
        return e.refers();
    }
    // A float3: three components, each a number or an expression, at least
    // one of which refers — `chf("../a/Size.x"):0:0`.
    let parts: Vec<&str> = t.split(':').collect();
    parts.len() == 3 && {
        let parsed: Vec<Option<Expr>> = parts.iter().map(|p| parse(p.trim()).ok()).collect();
        parsed.iter().all(Option::is_some) && parsed.iter().flatten().any(Expr::refers)
    }
}

impl Expr {
    /// Whether the expression reads a channel or a variable.
    pub fn refers(&self) -> bool {
        fn walk(n: &Node) -> bool {
            match n {
                Node::Ch(..) | Node::Var(_) => true,
                Node::Num(_) | Node::Str(_) => false,
                Node::Neg(a) | Node::Not(a) => walk(a),
                Node::Bin(_, a, b) => walk(a) || walk(b),
                Node::Call(_, args) => args.iter().any(walk),
            }
        }
        walk(&self.0)
    }

    /// Every channel path the expression names, in source order.
    pub fn paths(&self) -> Vec<&str> {
        fn walk<'a>(n: &'a Node, out: &mut Vec<&'a str>) {
            match n {
                Node::Ch(_, p) => out.push(p),
                Node::Num(_) | Node::Str(_) | Node::Var(_) => {}
                Node::Neg(a) | Node::Not(a) => walk(a, out),
                Node::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Node::Call(_, args) => args.iter().for_each(|a| walk(a, out)),
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &mut out);
        out
    }

    pub fn eval(&self, scope: &mut dyn Scope) -> Result<Value, String> {
        eval_node(&self.0, scope)
    }
}

fn eval_node(n: &Node, scope: &mut dyn Scope) -> Result<Value, String> {
    use Value::*;
    Ok(match n {
        Node::Num(v) => Num(*v),
        Node::Str(s) => Str(s.clone()),
        Node::Var(name) => scope.var(name).ok_or_else(|| format!("unknown variable ${name}"))?,
        Node::Neg(a) => Num(-eval_node(a, scope)?.as_num()),
        Node::Not(a) => Num(if eval_node(a, scope)?.truthy() { 0.0 } else { 1.0 }),
        Node::Ch(kind, path) => scope.channel(path, *kind)?,
        Node::Bin(op, a, b) => {
            // Short-circuit before evaluating the right side.
            match op {
                Op::And => {
                    let l = eval_node(a, scope)?;
                    return Ok(Num(if !l.truthy() { 0.0 } else if eval_node(b, scope)?.truthy() { 1.0 } else { 0.0 }));
                }
                Op::Or => {
                    let l = eval_node(a, scope)?;
                    return Ok(Num(if l.truthy() { 1.0 } else if eval_node(b, scope)?.truthy() { 1.0 } else { 0.0 }));
                }
                _ => {}
            }
            let l = eval_node(a, scope)?;
            let r = eval_node(b, scope)?;
            let both_str = matches!((&l, &r), (Str(_), _) | (_, Str(_)));
            match op {
                Op::Add if both_str => Str(format!("{}{}", l.as_str(), r.as_str())),
                Op::Eq if both_str => Num((l.as_str() == r.as_str()) as i32 as f64),
                Op::Ne if both_str => Num((l.as_str() != r.as_str()) as i32 as f64),
                _ => {
                    let (x, y) = (l.as_num(), r.as_num());
                    Num(match op {
                        Op::Add => x + y,
                        Op::Sub => x - y,
                        Op::Mul => x * y,
                        Op::Div => {
                            if y == 0.0 {
                                return Err("division by zero".to_string());
                            }
                            x / y
                        }
                        Op::Rem => {
                            if y == 0.0 {
                                return Err("modulo by zero".to_string());
                            }
                            x % y
                        }
                        Op::Pow => x.powf(y),
                        Op::Lt => (x < y) as i32 as f64,
                        Op::Le => (x <= y) as i32 as f64,
                        Op::Gt => (x > y) as i32 as f64,
                        Op::Ge => (x >= y) as i32 as f64,
                        Op::Eq => (x == y) as i32 as f64,
                        Op::Ne => (x != y) as i32 as f64,
                        Op::And | Op::Or => unreachable!(),
                    })
                }
            }
        }
        Node::Call(name, args) => call(name, args, scope)?,
    })
}

fn call(name: &str, args: &[Node], scope: &mut dyn Scope) -> Result<Value, String> {
    use Value::*;
    // `if` evaluates one branch only, so a guarded division stays guarded.
    if name == "if" {
        if args.len() != 3 {
            return Err("if() takes (condition, then, else)".to_string());
        }
        let c = eval_node(&args[0], scope)?;
        return eval_node(if c.truthy() { &args[1] } else { &args[2] }, scope);
    }
    let vals: Vec<Value> = args.iter().map(|a| eval_node(a, scope)).collect::<Result<_, _>>()?;
    let nums: Vec<f64> = vals.iter().map(Value::as_num).collect();
    let arity = |n: usize| -> Result<(), String> {
        if nums.len() == n { Ok(()) } else { Err(format!("{name}() takes {n} argument(s)")) }
    };
    let f1 = |f: fn(f64) -> f64| -> Result<Value, String> {
        arity(1)?;
        Ok(Num(f(nums[0])))
    };
    Ok(match name {
        "abs" => f1(f64::abs)?,
        "floor" => f1(f64::floor)?,
        "ceil" => f1(f64::ceil)?,
        "round" => f1(f64::round)?,
        "int" | "trunc" => f1(f64::trunc)?,
        "frac" => f1(f64::fract)?,
        "sqrt" => f1(f64::sqrt)?,
        "exp" => f1(f64::exp)?,
        "log" => f1(f64::ln)?,
        "sin" => f1(f64::sin)?,
        "cos" => f1(f64::cos)?,
        "tan" => f1(f64::tan)?,
        "asin" => f1(f64::asin)?,
        "acos" => f1(f64::acos)?,
        "atan" => f1(f64::atan)?,
        "sign" => f1(f64::signum)?,
        "atan2" => {
            arity(2)?;
            Num(nums[0].atan2(nums[1]))
        }
        "pow" => {
            arity(2)?;
            Num(nums[0].powf(nums[1]))
        }
        "min" => {
            if nums.is_empty() {
                return Err("min() takes at least one argument".to_string());
            }
            Num(nums.iter().cloned().fold(f64::INFINITY, f64::min))
        }
        "max" => {
            if nums.is_empty() {
                return Err("max() takes at least one argument".to_string());
            }
            Num(nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
        }
        "clamp" => {
            arity(3)?;
            Num(nums[0].max(nums[1]).min(nums[2]))
        }
        "lerp" => {
            arity(3)?;
            Num(nums[0] + (nums[1] - nums[0]) * nums[2])
        }
        // Houdini's fit: v in [omin, omax] mapped to [nmin, nmax], clamped.
        "fit" => {
            arity(5)?;
            let (v, omin, omax, nmin, nmax) = (nums[0], nums[1], nums[2], nums[3], nums[4]);
            let t = if omax == omin { 0.0 } else { ((v - omin) / (omax - omin)).clamp(0.0, 1.0) };
            Num(nmin + (nmax - nmin) * t)
        }
        // Deterministic in its seed, as Houdini's is: the same seed is the
        // same number on every evaluation and every machine.
        "rand" => {
            arity(1)?;
            let bits = nums[0].to_bits();
            let mut h = bits ^ 0x9E37_79B9_7F4A_7C15;
            h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            h ^= h >> 31;
            Num((h >> 11) as f64 / (1u64 << 53) as f64)
        }
        "strlen" => {
            arity(1)?;
            Num(vals[0].as_str().chars().count() as f64)
        }
        _ => return Err(format!("unknown function {name}()")),
    })
}

/// Rewrite every channel path in `src` through `f` — the textual counterpart
/// of [`Expr::paths`], for a rename: `f` gets each quoted path inside a
/// `ch*(...)` call and answers with its replacement, or None to leave it.
/// Textual rather than a re-print of the AST so the user's spacing and
/// spelling survive a rename of a node three levels away.
pub fn rewrite_paths(src: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A channel call: an identifier ch / chf / chi / chb / chs not
        // preceded by an identifier character, then `(`, then a string.
        let at_ident_start = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        let mut matched = None;
        if at_ident_start && bytes[i] == b'c' {
            for name in ["chf", "chi", "chb", "chs", "ch"] {
                if src[i..].starts_with(name) {
                    let rest = &src[i + name.len()..];
                    let trimmed = rest.trim_start();
                    if let Some(after_paren) = trimmed.strip_prefix('(') {
                        let inner = after_paren.trim_start();
                        if let Some(q) = inner.chars().next().filter(|c| *c == '"' || *c == '\'') {
                            let body = &inner[1..];
                            if let Some(end) = body.find(q) {
                                let path = &body[..end];
                                let consumed = name.len() + (rest.len() - trimmed.len()) + 1 + (after_paren.len() - inner.len()) + 1 + end + 1;
                                let prefix_len = consumed - end - 1;
                                matched = Some((prefix_len, path.to_string(), consumed, q));
                                break;
                            }
                        }
                    }
                }
            }
        }
        if let Some((prefix_len, path, consumed, q)) = matched {
            out.push_str(&src[i..i + prefix_len]);
            match f(&path) {
                Some(new) => out.push_str(&new),
                None => out.push_str(&path),
            }
            out.push(q);
            i += consumed;
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The pre-2026-09-24 reference: the WHOLE value one of `ch("Name")`,
/// `chf(...)`, `chi(...)`, `chb(...)`, with `../` per level — where a bare
/// name meant the PARENT's parameter. Kept for the load-time migration,
/// which turns it into an expression with Houdini's semantics (`../Name`).
pub fn parse_legacy_ref(value: &str) -> Option<(&'static str, String)> {
    let v = value.trim();
    let (kind, rest) = if let Some(r) = v.strip_prefix("chf(") {
        ("chf", r)
    } else if let Some(r) = v.strip_prefix("chi(") {
        ("chi", r)
    } else if let Some(r) = v.strip_prefix("chb(") {
        ("chb", r)
    } else if let Some(r) = v.strip_prefix("ch(") {
        ("ch", r)
    } else {
        return None;
    };
    let inner = rest.strip_suffix(')')?.trim();
    let quote = inner.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let path = inner.strip_prefix(quote)?.strip_suffix(quote)?;
    let mut name = path;
    while let Some(r) = name.strip_prefix("../") {
        name = r;
    }
    if name.trim().is_empty() || name.contains('/') {
        return None;
    }
    Some((kind, path.to_string()))
}

/// The legacy reference rewritten to Houdini's semantics: a bare name gains
/// `../` (it meant the parent), an explicit `../` path is already right.
pub fn migrate_legacy_ref(value: &str) -> Option<String> {
    let (kind, path) = parse_legacy_ref(value)?;
    let path = if path.starts_with("../") { path } else { format!("../{path}") };
    let mut s = String::new();
    let _ = write!(s, "{kind}(\"{path}\")");
    Some(s)
}
