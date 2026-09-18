//! CPU reference backend for the node kernels.
//!
//! The designer's kernel language is (a subset of) OpenCL C, which made every
//! kernel-generated node dependent on an OpenCL platform existing at runtime —
//! and "zero platforms" is a reachable state on this machine (nvidia-open out
//! of kernel lockstep, rusticl opt-in), reached silently: nodes just produced
//! empty geometry. This module is the reference implementation of that same
//! language on the CPU: a tree-walking interpreter for the C subset the
//! kernels use, executing the exact launcher contract of
//! `run_opencl_kernel_with_params` (same generator/deformer split, same
//! positional argument binding, same `max_vertices`, same output rebuild).
//!
//! It is the SEMANTIC REFERENCE: f32 arithmetic, C int truncation, postfix
//! increment, short-circuit logic — anything the GPU path disagrees with here
//! is a bug on one side or the other, and the cross-validation test compares
//! the two directly. Out-of-bounds buffer access — undefined behavior on the
//! GPU — reads 0 and drops writes here. A step budget guards the UI thread
//! against a kernel that never terminates (the GPU path would hang the queue
//! on that too; here it is a clean error instead).
//!
//! Deliberately NOT here: vector types (float3), barriers, local memory,
//! images — no shipped or plausible node kernel uses them. The interpreter
//! sees the same post-preprocessed source as OpenCL (`chf()` already rewritten
//! to `param_values[i]` by `preprocess_opencl_code`).

use crate::geometry::{GAttribute, GVertex, Geometry};
use std::collections::HashMap;

pub const MAX_VERTICES: usize = 200_000;
/// Interpreter step budget per kernel invocation (all work items together).
/// Generously above any real kernel (the sphere is ~100k steps, a full
/// 200k-vertex generator ~10M) while keeping a hung kernel's UI freeze in
/// seconds, not minutes.
const STEP_BUDGET: u64 = 50_000_000;

// ---------------------------------------------------------------- lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    Float(f32),
    Punct(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[s..i].to_string()));
        } else if c.is_ascii_digit() || (c == '.' && i + 1 < b.len() && (b[i + 1] as char).is_ascii_digit()) {
            let s = i;
            let mut is_float = false;
            while i < b.len() && (b[i] as char).is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                is_float = true;
                i += 1;
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                is_float = true;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            let text = &src[s..i];
            if i < b.len() && (b[i] == b'f' || b[i] == b'F') {
                is_float = true;
                i += 1;
            }
            if is_float {
                out.push(Tok::Float(text.parse::<f32>().map_err(|e| format!("bad float literal {text}: {e}"))?));
            } else {
                out.push(Tok::Int(text.parse::<i64>().map_err(|e| format!("bad int literal {text}: {e}"))?));
            }
        } else {
            // Longest-match puncts.
            const P3: [&str; 0] = [];
            const P2: [&str; 14] = ["==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "++", "--", "->"];
            let _ = P3;
            let rest = &src[i..];
            let mut matched = None;
            for p in P2 {
                if rest.starts_with(p) {
                    matched = Some(p);
                    break;
                }
            }
            if let Some(p) = matched {
                out.push(Tok::Punct(p));
                i += p.len();
            } else {
                const P1: &str = "+-*/%<>=!&|?:;,(){}[].";
                if P1.contains(c) {
                    let idx = P1.find(c).unwrap();
                    out.push(Tok::Punct(&P1[idx..idx + 1]));
                    i += 1;
                } else {
                    return Err(format!("kernel_cpu: unexpected character '{c}'"));
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- AST

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ty {
    Float,
    Int,
    Void,
}

#[derive(Debug, Clone)]
enum Expr {
    F(f32),
    I(i64),
    Var(String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    /// Postfix ++/-- (value is the OLD one).
    Post(&'static str, Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Assign(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Cast(Ty, Box<Expr>),
    AddrOf(Box<Expr>),
    Deref(Box<Expr>),
}

#[derive(Debug, Clone)]
enum Stmt {
    Decl { ty: Ty, name: String, arr: Option<usize>, init: Vec<Expr> },
    Expr(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Return(Option<Expr>),
    Break,
    Continue,
    Block(Vec<Stmt>),
    /// Comma-separated declarators (`float a = 1, b = 2;`): a sequence run in
    /// the CURRENT scope — Block would give the declarations their own scope
    /// and the names would vanish at the semicolon.
    Seq(Vec<Stmt>),
}

#[derive(Debug, Clone)]
struct Param {
    name: String,
    is_ptr: bool,
}

#[derive(Debug, Clone)]
struct FnDef {
    params: Vec<Param>,
    body: Vec<Stmt>,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(q)) if *q == p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, p: &str) -> Result<(), String> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(format!("kernel_cpu: expected '{p}' at token {:?}", self.peek()))
        }
    }
    fn eat_ident(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(q)) if q == s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    /// Skip OpenCL address-space and const qualifiers wherever they appear.
    fn skip_quals(&mut self) {
        loop {
            match self.peek() {
                Some(Tok::Ident(s))
                    if matches!(
                        s.as_str(),
                        "__kernel" | "kernel" | "__global" | "global" | "__constant" | "constant"
                            | "__local" | "local" | "__private" | "private" | "const" | "unsigned"
                    ) =>
                {
                    self.pos += 1;
                }
                _ => break,
            }
        }
    }
    fn peek_ty(&self) -> Option<Ty> {
        match self.peek() {
            Some(Tok::Ident(s)) => match s.as_str() {
                "float" => Some(Ty::Float),
                "int" | "bool" => Some(Ty::Int),
                "void" => Some(Ty::Void),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_program(&mut self) -> Result<HashMap<String, FnDef>, String> {
        let mut fns = HashMap::new();
        while self.peek().is_some() {
            self.skip_quals();
            let _ret = self.peek_ty().ok_or_else(|| format!("kernel_cpu: expected function return type, got {:?}", self.peek()))?;
            self.pos += 1;
            self.skip_quals();
            // Pointer return types don't occur; a stray '*' here would be one.
            let name = match self.next() {
                Some(Tok::Ident(n)) => n,
                t => return Err(format!("kernel_cpu: expected function name, got {t:?}")),
            };
            self.expect_punct("(")?;
            let mut params = Vec::new();
            if !self.eat_punct(")") {
                loop {
                    self.skip_quals();
                    if self.peek_ty().is_none() {
                        return Err(format!("kernel_cpu: expected parameter type, got {:?}", self.peek()));
                    }
                    self.pos += 1;
                    self.skip_quals();
                    let mut is_ptr = false;
                    while self.eat_punct("*") {
                        is_ptr = true;
                    }
                    self.skip_quals();
                    let pname = match self.next() {
                        Some(Tok::Ident(n)) => n,
                        t => return Err(format!("kernel_cpu: expected parameter name, got {t:?}")),
                    };
                    params.push(Param { name: pname, is_ptr });
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
            }
            self.expect_punct("{")?;
            let body = self.parse_block_body()?;
            fns.insert(name, FnDef { params, body });
        }
        Ok(fns)
    }

    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !self.eat_punct("}") {
            if self.peek().is_none() {
                return Err("kernel_cpu: unexpected end of source inside a block".into());
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        if self.eat_punct("{") {
            return Ok(Stmt::Block(self.parse_block_body()?));
        }
        if self.eat_ident("if") {
            self.expect_punct("(")?;
            let cond = self.parse_expr()?;
            self.expect_punct(")")?;
            let then = vec![self.parse_stmt()?];
            let els = if self.eat_ident("else") { vec![self.parse_stmt()?] } else { Vec::new() };
            return Ok(Stmt::If(cond, then, els));
        }
        if self.eat_ident("for") {
            self.expect_punct("(")?;
            let init = if self.eat_punct(";") { None } else { Some(Box::new(self.parse_stmt()?)) };
            let cond = if self.eat_punct(";") {
                None
            } else {
                let c = self.parse_expr()?;
                self.expect_punct(";")?;
                Some(c)
            };
            let step = if self.eat_punct(")") {
                None
            } else {
                let s = self.parse_expr()?;
                self.expect_punct(")")?;
                Some(s)
            };
            let body = vec![self.parse_stmt()?];
            return Ok(Stmt::For(init, cond, step, body));
        }
        if self.eat_ident("while") {
            self.expect_punct("(")?;
            let cond = self.parse_expr()?;
            self.expect_punct(")")?;
            let body = vec![self.parse_stmt()?];
            return Ok(Stmt::While(cond, body));
        }
        if self.eat_ident("return") {
            if self.eat_punct(";") {
                return Ok(Stmt::Return(None));
            }
            let e = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::Return(Some(e)));
        }
        if self.eat_ident("break") {
            self.expect_punct(";")?;
            return Ok(Stmt::Break);
        }
        if self.eat_ident("continue") {
            self.expect_punct(";")?;
            return Ok(Stmt::Continue);
        }
        self.skip_quals();
        if let Some(ty) = self.peek_ty() {
            // Declaration — possibly several declarators (`float a = 1, b;`).
            self.pos += 1;
            let mut decls = Vec::new();
            loop {
                self.skip_quals();
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    t => return Err(format!("kernel_cpu: expected variable name, got {t:?}")),
                };
                let mut arr = None;
                if self.eat_punct("[") {
                    match self.next() {
                        Some(Tok::Int(n)) => arr = Some(n as usize),
                        t => return Err(format!("kernel_cpu: expected array length, got {t:?}")),
                    }
                    self.expect_punct("]")?;
                }
                let mut init = Vec::new();
                if self.eat_punct("=") {
                    if self.eat_punct("{") {
                        if !self.eat_punct("}") {
                            loop {
                                init.push(self.parse_assign()?);
                                if !self.eat_punct(",") {
                                    break;
                                }
                            }
                            self.expect_punct("}")?;
                        }
                    } else {
                        init.push(self.parse_assign()?);
                    }
                }
                decls.push(Stmt::Decl { ty, name, arr, init });
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(";")?;
            return Ok(if decls.len() == 1 { decls.pop().unwrap() } else { Stmt::Seq(decls) });
        }
        let e = self.parse_expr()?;
        self.expect_punct(";")?;
        Ok(Stmt::Expr(e))
    }

    // Expression grammar: comma-free C precedence.
    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_assign()
    }
    fn parse_assign(&mut self) -> Result<Expr, String> {
        let lhs = self.parse_ternary()?;
        for op in ["=", "+=", "-=", "*=", "/=", "%="] {
            if matches!(self.peek(), Some(Tok::Punct(p)) if *p == op) {
                self.pos += 1;
                let rhs = self.parse_assign()?;
                let sop: &'static str = match op {
                    "=" => "=",
                    "+=" => "+=",
                    "-=" => "-=",
                    "*=" => "*=",
                    "/=" => "/=",
                    _ => "%=",
                };
                return Ok(Expr::Assign(sop, Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }
    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let cond = self.parse_bin(0)?;
        if self.eat_punct("?") {
            let a = self.parse_assign()?;
            self.expect_punct(":")?;
            let b = self.parse_assign()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }
    fn parse_bin(&mut self, min_prec: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, prec): (&'static str, u8) = match self.peek() {
                Some(Tok::Punct(p)) => match *p {
                    "||" => ("||", 1),
                    "&&" => ("&&", 2),
                    "==" => ("==", 3),
                    "!=" => ("!=", 3),
                    "<" => ("<", 4),
                    ">" => (">", 4),
                    "<=" => ("<=", 4),
                    ">=" => (">=", 4),
                    "+" => ("+", 5),
                    "-" => ("-", 5),
                    "*" => ("*", 6),
                    "/" => ("/", 6),
                    "%" => ("%", 6),
                    _ => break,
                },
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_bin(prec + 1)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_unary(&mut self) -> Result<Expr, String> {
        // Cast: '(' type ')' unary — only for our two value types.
        if matches!(self.peek(), Some(Tok::Punct("("))) {
            if let Some(Tok::Ident(s)) = self.toks.get(self.pos + 1) {
                let ty = match s.as_str() {
                    "float" => Some(Ty::Float),
                    "int" => Some(Ty::Int),
                    _ => None,
                };
                if ty.is_some() && matches!(self.toks.get(self.pos + 2), Some(Tok::Punct(")"))) {
                    self.pos += 3;
                    let e = self.parse_unary()?;
                    return Ok(Expr::Cast(ty.unwrap(), Box::new(e)));
                }
            }
        }
        if self.eat_punct("-") {
            return Ok(Expr::Unary("-", Box::new(self.parse_unary()?)));
        }
        if self.eat_punct("!") {
            return Ok(Expr::Unary("!", Box::new(self.parse_unary()?)));
        }
        if self.eat_punct("+") {
            return self.parse_unary();
        }
        if self.eat_punct("&") {
            return Ok(Expr::AddrOf(Box::new(self.parse_unary()?)));
        }
        if self.eat_punct("*") {
            return Ok(Expr::Deref(Box::new(self.parse_unary()?)));
        }
        if self.eat_punct("++") {
            // Prefix inc as `x += 1`.
            let e = self.parse_unary()?;
            return Ok(Expr::Assign("+=", Box::new(e), Box::new(Expr::I(1))));
        }
        if self.eat_punct("--") {
            let e = self.parse_unary()?;
            return Ok(Expr::Assign("-=", Box::new(e), Box::new(Expr::I(1))));
        }
        self.parse_postfix()
    }
    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.parse_primary()?;
        loop {
            if self.eat_punct("[") {
                let idx = self.parse_expr()?;
                self.expect_punct("]")?;
                e = Expr::Index(Box::new(e), Box::new(idx));
            } else if matches!(self.peek(), Some(Tok::Punct("++"))) {
                self.pos += 1;
                e = Expr::Post("++", Box::new(e));
            } else if matches!(self.peek(), Some(Tok::Punct("--"))) {
                self.pos += 1;
                e = Expr::Post("--", Box::new(e));
            } else {
                break;
            }
        }
        Ok(e)
    }
    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::I(n)),
            Some(Tok::Float(f)) => Ok(Expr::F(f)),
            Some(Tok::Punct("(")) => {
                let e = self.parse_expr()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                if name == "true" {
                    return Ok(Expr::I(1));
                }
                if name == "false" {
                    return Ok(Expr::I(0));
                }
                if self.eat_punct("(") {
                    let mut args = Vec::new();
                    if !self.eat_punct(")") {
                        loop {
                            args.push(self.parse_assign()?);
                            if !self.eat_punct(",") {
                                break;
                            }
                        }
                        self.expect_punct(")")?;
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            t => Err(format!("kernel_cpu: unexpected token {t:?} in expression")),
        }
    }
}

// ---------------------------------------------------------------- interpreter

#[derive(Debug, Clone, Copy, PartialEq)]
enum BufId {
    InPos,
    InCol,
    OutPos,
    OutCol,
    OutCount,
    Params,
    RwPos,
    RwCol,
    /// One of the kernel's named attribute buffers, by binding slot — the
    /// Phase 1 ABI's addition. See `geometry::parse_attr_refs`.
    Attr(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PtrV {
    Slot(usize),
    Buf(BufId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Val {
    F(f32),
    I(i64),
    P(PtrV),
}

impl Val {
    fn as_f(self) -> f32 {
        match self {
            Val::F(f) => f,
            Val::I(i) => i as f32,
            Val::P(_) => 0.0,
        }
    }
    fn as_i(self) -> i64 {
        match self {
            Val::F(f) => f as i64, // C float→int truncates toward zero, as `as` does
            Val::I(i) => i,
            Val::P(_) => 0,
        }
    }
    fn truthy(self) -> bool {
        match self {
            Val::F(f) => f != 0.0,
            Val::I(i) => i != 0,
            Val::P(_) => true,
        }
    }
}

#[derive(Debug, Clone)]
enum SlotData {
    F(f32),
    I(i64),
    ArrF(Vec<f32>),
    ArrI(Vec<i64>),
    Ptr(PtrV),
}

/// An assignable location, resolved before the store.
enum Place {
    Slot(usize),
    SlotElem(usize, usize),
    BufElem(BufId, usize),
}

enum Flow {
    Normal,
    Break,
    Continue,
    Return(Option<Val>),
}

struct Bufs<'a> {
    in_pos: &'a [f32],
    in_col: &'a [f32],
    out_pos: Vec<f32>,
    out_col: Vec<f32>,
    out_count: i64,
    rw_pos: Vec<f32>,
    rw_col: Vec<f32>,
    params: &'a [f32],
    attrs: Vec<Vec<f32>>,
}

struct Interp<'a> {
    fns: &'a HashMap<String, FnDef>,
    slots: Vec<SlotData>,
    scopes: Vec<HashMap<String, usize>>,
    bufs: Bufs<'a>,
    global_id: usize,
    steps: u64,
    budget: u64,
}

impl<'a> Interp<'a> {
    fn step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > self.budget {
            Err("kernel step budget exceeded (non-terminating kernel?)".into())
        } else {
            Ok(())
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, data: SlotData) -> usize {
        let idx = self.slots.len();
        self.slots.push(data);
        self.scopes.last_mut().unwrap().insert(name.to_string(), idx);
        idx
    }
    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(&i) = scope.get(name) {
                return Some(i);
            }
        }
        None
    }

    fn buf_read(&self, id: BufId, idx: usize) -> Val {
        match id {
            BufId::InPos => Val::F(self.bufs.in_pos.get(idx).copied().unwrap_or(0.0)),
            BufId::InCol => Val::F(self.bufs.in_col.get(idx).copied().unwrap_or(0.0)),
            BufId::OutPos => Val::F(self.bufs.out_pos.get(idx).copied().unwrap_or(0.0)),
            BufId::OutCol => Val::F(self.bufs.out_col.get(idx).copied().unwrap_or(0.0)),
            BufId::OutCount => Val::I(if idx == 0 { self.bufs.out_count } else { 0 }),
            BufId::Params => Val::F(self.bufs.params.get(idx).copied().unwrap_or(0.0)),
            BufId::RwPos => Val::F(self.bufs.rw_pos.get(idx).copied().unwrap_or(0.0)),
            BufId::RwCol => Val::F(self.bufs.rw_col.get(idx).copied().unwrap_or(0.0)),
            BufId::Attr(slot) => Val::F(
                self.bufs
                    .attrs
                    .get(slot)
                    .and_then(|a| a.get(idx))
                    .copied()
                    .unwrap_or(0.0),
            ),
        }
    }
    fn buf_write(&mut self, id: BufId, idx: usize, v: Val) {
        // OOB writes are dropped — the GPU's UB made safe.
        match id {
            BufId::OutPos => {
                if let Some(slot) = self.bufs.out_pos.get_mut(idx) {
                    *slot = v.as_f();
                }
            }
            BufId::OutCol => {
                if let Some(slot) = self.bufs.out_col.get_mut(idx) {
                    *slot = v.as_f();
                }
            }
            BufId::OutCount => {
                if idx == 0 {
                    self.bufs.out_count = v.as_i();
                }
            }
            BufId::RwPos => {
                if let Some(slot) = self.bufs.rw_pos.get_mut(idx) {
                    *slot = v.as_f();
                }
            }
            BufId::RwCol => {
                if let Some(slot) = self.bufs.rw_col.get_mut(idx) {
                    *slot = v.as_f();
                }
            }
            BufId::Attr(slot) => {
                if let Some(slot) = self.bufs.attrs.get_mut(slot).and_then(|a| a.get_mut(idx)) {
                    *slot = v.as_f();
                }
            }
            BufId::InPos | BufId::InCol | BufId::Params => {}
        }
    }

    fn place_read(&self, p: &Place) -> Val {
        match p {
            Place::Slot(i) => match &self.slots[*i] {
                SlotData::F(f) => Val::F(*f),
                SlotData::I(n) => Val::I(*n),
                SlotData::Ptr(p) => Val::P(*p),
                SlotData::ArrF(_) | SlotData::ArrI(_) => Val::I(0),
            },
            Place::SlotElem(i, k) => match &self.slots[*i] {
                SlotData::ArrF(v) => Val::F(v.get(*k).copied().unwrap_or(0.0)),
                SlotData::ArrI(v) => Val::I(v.get(*k).copied().unwrap_or(0)),
                _ => Val::I(0),
            },
            Place::BufElem(id, k) => self.buf_read(*id, *k),
        }
    }
    fn place_write(&mut self, p: &Place, v: Val) {
        match p {
            Place::Slot(i) => {
                let slot = &mut self.slots[*i];
                match slot {
                    SlotData::F(f) => *f = v.as_f(),
                    SlotData::I(n) => *n = v.as_i(),
                    SlotData::Ptr(q) => {
                        if let Val::P(np) = v {
                            *q = np;
                        }
                    }
                    SlotData::ArrF(_) | SlotData::ArrI(_) => {}
                }
            }
            Place::SlotElem(i, k) => {
                let slot = &mut self.slots[*i];
                match slot {
                    SlotData::ArrF(vec) => {
                        if let Some(e) = vec.get_mut(*k) {
                            *e = v.as_f();
                        }
                    }
                    SlotData::ArrI(vec) => {
                        if let Some(e) = vec.get_mut(*k) {
                            *e = v.as_i();
                        }
                    }
                    _ => {}
                }
            }
            Place::BufElem(id, k) => self.buf_write(*id, *k, v),
        }
    }

    /// Resolve an lvalue expression to a storage location.
    fn resolve_place(&mut self, e: &Expr) -> Result<Place, String> {
        match e {
            Expr::Var(name) => {
                let idx = self.lookup(name).ok_or_else(|| format!("kernel_cpu: unknown variable '{name}'"))?;
                Ok(Place::Slot(idx))
            }
            Expr::Index(base, idx) => {
                let k = self.eval(idx)?.as_i();
                if k < 0 {
                    return Ok(Place::BufElem(BufId::Params, usize::MAX)); // negative index: dead place
                }
                let k = k as usize;
                match &**base {
                    Expr::Var(name) => {
                        let slot_idx = self.lookup(name).ok_or_else(|| format!("kernel_cpu: unknown variable '{name}'"))?;
                        match &self.slots[slot_idx] {
                            SlotData::Ptr(PtrV::Buf(id)) => Ok(Place::BufElem(*id, k)),
                            SlotData::Ptr(PtrV::Slot(s)) => Ok(Place::SlotElem(*s, k)),
                            SlotData::ArrF(_) | SlotData::ArrI(_) => Ok(Place::SlotElem(slot_idx, k)),
                            _ => Err(format!("kernel_cpu: '{name}' is not indexable")),
                        }
                    }
                    other => {
                        // e.g. (*ptr)[k] — evaluate to a pointer and index it.
                        let v = self.eval(other)?;
                        match v {
                            Val::P(PtrV::Buf(id)) => Ok(Place::BufElem(id, k)),
                            Val::P(PtrV::Slot(s)) => Ok(Place::SlotElem(s, k)),
                            _ => Err("kernel_cpu: indexing a non-pointer expression".into()),
                        }
                    }
                }
            }
            Expr::Deref(inner) => {
                let v = self.eval(inner)?;
                match v {
                    Val::P(PtrV::Buf(id)) => Ok(Place::BufElem(id, 0)),
                    Val::P(PtrV::Slot(s)) => Ok(Place::Slot(s)),
                    _ => Err("kernel_cpu: dereferencing a non-pointer".into()),
                }
            }
            _ => Err("kernel_cpu: expression is not assignable".into()),
        }
    }

    fn eval(&mut self, e: &Expr) -> Result<Val, String> {
        self.step()?;
        match e {
            Expr::F(f) => Ok(Val::F(*f)),
            Expr::I(n) => Ok(Val::I(*n)),
            Expr::Var(name) => {
                let idx = self.lookup(name).ok_or_else(|| format!("kernel_cpu: unknown variable '{name}'"))?;
                Ok(self.place_read(&Place::Slot(idx)))
            }
            Expr::Index(..) | Expr::Deref(..) => {
                let p = self.resolve_place(e)?;
                Ok(self.place_read(&p))
            }
            Expr::AddrOf(inner) => match self.resolve_place(inner)? {
                Place::Slot(i) => Ok(Val::P(PtrV::Slot(i))),
                Place::BufElem(id, 0) => Ok(Val::P(PtrV::Buf(id))),
                _ => Err("kernel_cpu: unsupported address-of".into()),
            },
            Expr::Unary(op, inner) => {
                let v = self.eval(inner)?;
                match (*op, v) {
                    ("-", Val::F(f)) => Ok(Val::F(-f)),
                    ("-", Val::I(n)) => Ok(Val::I(n.wrapping_neg())),
                    ("!", v) => Ok(Val::I(if v.truthy() { 0 } else { 1 })),
                    _ => Err(format!("kernel_cpu: bad unary {op}")),
                }
            }
            Expr::Post(op, inner) => {
                let p = self.resolve_place(inner)?;
                let old = self.place_read(&p);
                let one = Val::I(1);
                let new = bin_arith(if *op == "++" { "+" } else { "-" }, old, one)?;
                self.place_write(&p, new);
                Ok(old)
            }
            Expr::Bin(op, a, b) => match *op {
                "&&" => {
                    let va = self.eval(a)?;
                    if !va.truthy() {
                        return Ok(Val::I(0));
                    }
                    Ok(Val::I(if self.eval(b)?.truthy() { 1 } else { 0 }))
                }
                "||" => {
                    let va = self.eval(a)?;
                    if va.truthy() {
                        return Ok(Val::I(1));
                    }
                    Ok(Val::I(if self.eval(b)?.truthy() { 1 } else { 0 }))
                }
                _ => {
                    let va = self.eval(a)?;
                    let vb = self.eval(b)?;
                    bin_arith(op, va, vb)
                }
            },
            Expr::Assign(op, lhs, rhs) => {
                let rv = self.eval(rhs)?;
                let p = self.resolve_place(lhs)?;
                let out = if *op == "=" {
                    rv
                } else {
                    let cur = self.place_read(&p);
                    let bop = &op[..1]; // "+=" -> "+"
                    bin_arith(bop, cur, rv)?
                };
                self.place_write(&p, out);
                Ok(self.place_read(&p))
            }
            Expr::Ternary(c, a, b) => {
                if self.eval(c)?.truthy() {
                    self.eval(a)
                } else {
                    self.eval(b)
                }
            }
            Expr::Cast(ty, inner) => {
                let v = self.eval(inner)?;
                Ok(match ty {
                    Ty::Float => Val::F(v.as_f()),
                    Ty::Int => Val::I(v.as_i()),
                    Ty::Void => Val::I(0),
                })
            }
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<Val, String> {
        // Builtins first.
        match name {
            "get_global_id" => return Ok(Val::I(self.global_id as i64)),
            "get_global_size" => return Ok(Val::I(1)),
            _ => {}
        }
        if let Some(v) = self.try_math_builtin(name, args)? {
            return Ok(v);
        }
        let def = self
            .fns
            .get(name)
            .ok_or_else(|| format!("kernel_cpu: unknown function '{name}'"))?
            .clone();
        if def.params.len() != args.len() {
            return Err(format!("kernel_cpu: {name} expects {} args, got {}", def.params.len(), args.len()));
        }
        let mut bound = Vec::with_capacity(args.len());
        for (param, arg) in def.params.iter().zip(args) {
            let v = if param.is_ptr {
                // Pointer parameter: pass a pointer value; a bare buffer/array
                // name decays to a pointer to it.
                match arg {
                    Expr::Var(n) => {
                        let idx = self.lookup(n).ok_or_else(|| format!("kernel_cpu: unknown variable '{n}'"))?;
                        match &self.slots[idx] {
                            SlotData::Ptr(p) => Val::P(*p),
                            SlotData::ArrF(_) | SlotData::ArrI(_) => Val::P(PtrV::Slot(idx)),
                            _ => Val::P(PtrV::Slot(idx)),
                        }
                    }
                    _ => self.eval(arg)?,
                }
            } else {
                self.eval(arg)?
            };
            bound.push((param.name.clone(), param.is_ptr, v));
        }
        self.push_scope();
        for (pname, is_ptr, v) in bound {
            let data = if is_ptr {
                match v {
                    Val::P(p) => SlotData::Ptr(p),
                    _ => return Err(format!("kernel_cpu: pointer argument '{pname}' is not a pointer")),
                }
            } else {
                match v {
                    Val::F(f) => SlotData::F(f),
                    Val::I(n) => SlotData::I(n),
                    Val::P(p) => SlotData::Ptr(p),
                }
            };
            self.declare(&pname, data);
        }
        let flow = self.run_block(&def.body)?;
        self.pop_scope();
        match flow {
            Flow::Return(Some(v)) => Ok(v),
            _ => Ok(Val::I(0)),
        }
    }

    fn try_math_builtin(&mut self, name: &str, args: &[Expr]) -> Result<Option<Val>, String> {
        let f1 = |i: &mut Self, args: &[Expr]| -> Result<f32, String> { Ok(i.eval(&args[0])?.as_f()) };
        let v = match (name, args.len()) {
            ("sqrt", 1) => Val::F(f1(self, args)?.sqrt()),
            ("sin", 1) => Val::F(f1(self, args)?.sin()),
            ("cos", 1) => Val::F(f1(self, args)?.cos()),
            ("tan", 1) => Val::F(f1(self, args)?.tan()),
            ("fabs", 1) => Val::F(f1(self, args)?.abs()),
            ("floor", 1) => Val::F(f1(self, args)?.floor()),
            ("ceil", 1) => Val::F(f1(self, args)?.ceil()),
            ("exp", 1) => Val::F(f1(self, args)?.exp()),
            ("log", 1) => Val::F(f1(self, args)?.ln()),
            ("abs", 1) => Val::I(self.eval(&args[0])?.as_i().wrapping_abs()),
            ("fmod", 2) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                Val::F(a % b)
            }
            ("pow", 2) | ("powr", 2) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                Val::F(a.powf(b))
            }
            ("atan2", 2) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                Val::F(a.atan2(b))
            }
            ("fmin", 2) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                Val::F(a.min(b))
            }
            ("fmax", 2) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                Val::F(a.max(b))
            }
            ("min", 2) => {
                let a = self.eval(&args[0])?;
                let b = self.eval(&args[1])?;
                match (a, b) {
                    (Val::I(x), Val::I(y)) => Val::I(x.min(y)),
                    _ => Val::F(a.as_f().min(b.as_f())),
                }
            }
            ("max", 2) => {
                let a = self.eval(&args[0])?;
                let b = self.eval(&args[1])?;
                match (a, b) {
                    (Val::I(x), Val::I(y)) => Val::I(x.max(y)),
                    _ => Val::F(a.as_f().max(b.as_f())),
                }
            }
            ("clamp", 3) => {
                let x = self.eval(&args[0])?.as_f();
                let lo = self.eval(&args[1])?.as_f();
                let hi = self.eval(&args[2])?.as_f();
                Val::F(x.clamp(lo, hi))
            }
            ("mix", 3) => {
                let a = self.eval(&args[0])?.as_f();
                let b = self.eval(&args[1])?.as_f();
                let t = self.eval(&args[2])?.as_f();
                Val::F(a + (b - a) * t)
            }
            _ => return Ok(None),
        };
        Ok(Some(v))
    }

    fn run_block(&mut self, stmts: &[Stmt]) -> Result<Flow, String> {
        self.push_scope();
        let mut flow = Flow::Normal;
        for s in stmts {
            flow = self.run_stmt(s)?;
            if !matches!(flow, Flow::Normal) {
                break;
            }
        }
        self.pop_scope();
        Ok(flow)
    }

    fn run_stmt(&mut self, s: &Stmt) -> Result<Flow, String> {
        self.step()?;
        match s {
            Stmt::Block(body) => self.run_block(body),
            Stmt::Seq(stmts) => {
                for st in stmts {
                    match self.run_stmt(st)? {
                        Flow::Normal => {}
                        f => return Ok(f),
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::Expr(e) => {
                self.eval(e)?;
                Ok(Flow::Normal)
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => Some(self.eval(e)?),
                    None => None,
                };
                Ok(Flow::Return(v))
            }
            Stmt::Break => Ok(Flow::Break),
            Stmt::Continue => Ok(Flow::Continue),
            Stmt::If(c, t, f) => {
                if self.eval(c)?.truthy() {
                    self.run_block(t)
                } else {
                    self.run_block(f)
                }
            }
            Stmt::While(c, body) => {
                loop {
                    if !self.eval(c)?.truthy() {
                        break;
                    }
                    match self.run_block(body)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::For(init, cond, step, body) => {
                self.push_scope();
                if let Some(init) = init {
                    self.run_stmt(init)?;
                }
                loop {
                    if let Some(c) = cond {
                        if !self.eval(c)?.truthy() {
                            break;
                        }
                    }
                    match self.run_block(body)? {
                        Flow::Break => break,
                        Flow::Return(v) => {
                            self.pop_scope();
                            return Ok(Flow::Return(v));
                        }
                        _ => {}
                    }
                    if let Some(st) = step {
                        self.eval(st)?;
                    }
                }
                self.pop_scope();
                Ok(Flow::Normal)
            }
            Stmt::Decl { ty, name, arr, init } => {
                let data = match (arr, ty) {
                    (Some(n), Ty::Float) => {
                        let mut v = vec![0.0f32; *n];
                        for (i, e) in init.iter().enumerate().take(*n) {
                            v[i] = self.eval(e)?.as_f();
                        }
                        SlotData::ArrF(v)
                    }
                    (Some(n), _) => {
                        let mut v = vec![0i64; *n];
                        for (i, e) in init.iter().enumerate().take(*n) {
                            v[i] = self.eval(e)?.as_i();
                        }
                        SlotData::ArrI(v)
                    }
                    (None, Ty::Float) => {
                        let v = init.first().map(|e| self.eval(e)).transpose()?.map(|v| v.as_f()).unwrap_or(0.0);
                        SlotData::F(v)
                    }
                    (None, _) => match init.first().map(|e| self.eval(e)).transpose()? {
                        Some(Val::P(p)) => SlotData::Ptr(p),
                        Some(v) => SlotData::I(v.as_i()),
                        None => SlotData::I(0),
                    },
                };
                self.declare(name, data);
                Ok(Flow::Normal)
            }
        }
    }
}

/// C arithmetic: int op int stays int (truncating division, wrapping i32-ish),
/// anything touching a float promotes to f32; comparisons yield int 0/1.
fn bin_arith(op: &str, a: Val, b: Val) -> Result<Val, String> {
    let both_int = matches!(a, Val::I(_)) && matches!(b, Val::I(_));
    Ok(match op {
        "+" | "-" | "*" | "/" | "%" => {
            if both_int {
                let (x, y) = (a.as_i(), b.as_i());
                let r = match op {
                    "+" => x.wrapping_add(y),
                    "-" => x.wrapping_sub(y),
                    "*" => x.wrapping_mul(y),
                    "/" => {
                        if y == 0 {
                            0
                        } else {
                            x.wrapping_div(y)
                        }
                    }
                    _ => {
                        if y == 0 {
                            0
                        } else {
                            x.wrapping_rem(y)
                        }
                    }
                };
                Val::I(r)
            } else {
                let (x, y) = (a.as_f(), b.as_f());
                let r = match op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    "/" => x / y,
                    _ => x % y,
                };
                Val::F(r)
            }
        }
        "<" | ">" | "<=" | ">=" | "==" | "!=" => {
            let t = if both_int {
                let (x, y) = (a.as_i(), b.as_i());
                match op {
                    "<" => x < y,
                    ">" => x > y,
                    "<=" => x <= y,
                    ">=" => x >= y,
                    "==" => x == y,
                    _ => x != y,
                }
            } else {
                let (x, y) = (a.as_f(), b.as_f());
                match op {
                    "<" => x < y,
                    ">" => x > y,
                    "<=" => x <= y,
                    ">=" => x >= y,
                    "==" => x == y,
                    _ => x != y,
                }
            };
            Val::I(if t { 1 } else { 0 })
        }
        _ => return Err(format!("kernel_cpu: unknown operator {op}")),
    })
}

// ---------------------------------------------------------------- launcher

/// CPU twin of `run_opencl_kernel_with_params`: same generator/deformer split
/// (a kernel that mentions `out_count` is a generator), same positional
/// argument binding, same `max_vertices`, same output rebuild with the default
/// Norm/UV attributes. The `process` entry point runs once per work item with
/// `get_global_id(0)` = the item index, exactly as the ND-range launch does.
/// CPU twin of `geometry::run_deformer_flat`: same flat buffers, same binding
/// order, same read-back in place.
///
/// The two backends share the kernel language, so they have to share the ABI
/// too — `cpu_matches_opencl_on_every_shipped_kernel` is only meaningful while
/// they bind the same arguments to the same slots.
pub fn run_deformer_cpu(
    code: &str,
    pos: &mut [f32],
    col: &mut [f32],
    count: usize,
    attrs: &mut [(String, Vec<f32>)],
    params: &[f32],
) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    let toks = lex(code)?;
    let mut parser = Parser { toks, pos: 0 };
    let fns = parser.parse_program()?;
    let process = fns.get("process").ok_or("kernel_cpu: no 'process' kernel")?;
    let arity = process.params.len();
    let pnames: Vec<(String, bool)> = process
        .params
        .iter()
        .map(|p| (p.name.clone(), p.is_ptr))
        .collect();

    let mut param_data = params.to_vec();
    if param_data.is_empty() {
        param_data.push(0.0);
    }

    // Binding order mirrors the OpenCL launcher exactly: the three fixed
    // arguments, `param_values` only when the arity says the kernel declared
    // it, then one buffer per named attribute.
    let mut binds: Vec<Val> = vec![
        Val::P(PtrV::Buf(BufId::RwPos)),
        Val::P(PtrV::Buf(BufId::RwCol)),
        Val::I(count as i64),
    ];
    if arity > 3 + attrs.len() {
        binds.push(Val::P(PtrV::Buf(BufId::Params)));
    }
    for slot in 0..attrs.len() {
        binds.push(Val::P(PtrV::Buf(BufId::Attr(slot))));
    }
    let n_bind = binds.len().min(arity);

    let mut bufs = Bufs {
        in_pos: &[],
        in_col: &[],
        out_pos: Vec::new(),
        out_col: Vec::new(),
        out_count: 0,
        rw_pos: pos.to_vec(),
        rw_col: col.to_vec(),
        params: &param_data,
        attrs: attrs.iter().map(|(_, v)| v.clone()).collect(),
    };

    let mut steps = 0u64;
    for id in 0..count {
        let mut interp = Interp {
            fns: &fns,
            slots: Vec::new(),
            scopes: vec![HashMap::new()],
            bufs: std::mem::replace(
                &mut bufs,
                Bufs { in_pos: &[], in_col: &[], out_pos: Vec::new(), out_col: Vec::new(), out_count: 0, rw_pos: Vec::new(), rw_col: Vec::new(), params: &[], attrs: Vec::new() },
            ),
            global_id: id,
            steps,
            budget: STEP_BUDGET,
        };
        for (i, (name, is_ptr)) in pnames.iter().enumerate().take(n_bind) {
            let data = match binds[i] {
                Val::P(p) if *is_ptr => SlotData::Ptr(p),
                Val::I(n) => SlotData::I(n),
                Val::F(f) => SlotData::F(f),
                Val::P(p) => SlotData::Ptr(p),
            };
            interp.declare(name, data);
        }
        interp.run_block(&process.body)?;
        steps = interp.steps;
        bufs = interp.bufs;
    }

    pos.copy_from_slice(&bufs.rw_pos);
    col.copy_from_slice(&bufs.rw_col);
    for ((_, dst), src) in attrs.iter_mut().zip(bufs.attrs.into_iter()) {
        *dst = src;
    }
    Ok(())
}

pub fn run_kernel_cpu(code: &str, geom: &mut Geometry, params: &[f32]) -> Result<(), String> {
    run_kernel_cpu_with_budget(code, geom, params, STEP_BUDGET)
}

fn run_kernel_cpu_with_budget(code: &str, geom: &mut Geometry, params: &[f32], budget: u64) -> Result<(), String> {
    let is_generator = code.contains("out_count");
    if geom.vertices.is_empty() && !is_generator {
        return Ok(());
    }

    let toks = lex(code)?;
    let mut parser = Parser { toks, pos: 0 };
    let fns = parser.parse_program()?;
    let process = fns.get("process").ok_or("kernel_cpu: no 'process' kernel")?;
    let arity = process.params.len();
    let pnames: Vec<(String, bool)> = process.params.iter().map(|p| (p.name.clone(), p.is_ptr)).collect();

    let mut param_data = params.to_vec();
    if param_data.is_empty() {
        param_data.push(0.0);
    }

    let mut in_pos = Vec::new();
    let mut in_col = Vec::new();
    for v in &geom.vertices {
        in_pos.extend_from_slice(&v.pos);
        in_col.extend_from_slice(&v.col);
    }
    let count = geom.vertices.len();

    if is_generator {
        // Positional binding mirrors the OpenCL set_arg order; param_values is
        // bound only when the kernel declares the extra argument, exactly like
        // the launcher's num_args >= 8 check.
        let binds: Vec<Val> = vec![
            Val::P(PtrV::Buf(BufId::InPos)),
            Val::P(PtrV::Buf(BufId::InCol)),
            Val::I(count as i64),
            Val::P(PtrV::Buf(BufId::OutPos)),
            Val::P(PtrV::Buf(BufId::OutCol)),
            Val::P(PtrV::Buf(BufId::OutCount)),
            Val::I(MAX_VERTICES as i64),
            Val::P(PtrV::Buf(BufId::Params)),
        ];
        let n_bind = if arity >= 8 { 8 } else { 7.min(arity) };

        let mut bufs = Bufs {
            in_pos: &in_pos,
            in_col: &in_col,
            out_pos: vec![0.0; MAX_VERTICES * 3],
            out_col: vec![0.0; MAX_VERTICES * 3],
            out_count: 0,
            rw_pos: Vec::new(),
            rw_col: Vec::new(),
            params: &param_data,
            attrs: Vec::new(),
        };

        let global = count.max(1);
        let mut steps = 0u64;
        for id in 0..global {
            let mut interp = Interp {
                fns: &fns,
                slots: Vec::new(),
                scopes: vec![HashMap::new()],
                bufs: std::mem::replace(
                    &mut bufs,
                    Bufs { in_pos: &[], in_col: &[], out_pos: Vec::new(), out_col: Vec::new(), out_count: 0, rw_pos: Vec::new(), rw_col: Vec::new(), params: &[], attrs: Vec::new() },
                ),
                global_id: id,
                steps,
                budget,
            };
            for (i, (name, is_ptr)) in pnames.iter().enumerate().take(n_bind) {
                let data = match binds[i] {
                    Val::P(p) if *is_ptr => SlotData::Ptr(p),
                    Val::I(n) => SlotData::I(n),
                    Val::F(f) => SlotData::F(f),
                    Val::P(p) => SlotData::Ptr(p),
                };
                interp.declare(name, data);
            }
            let flow = interp.run_block(&process.body)?;
            let _ = flow;
            steps = interp.steps;
            bufs = interp.bufs;
        }

        let final_count = (bufs.out_count.max(0) as usize).min(MAX_VERTICES);
        geom.vertices.clear();
        for i in 0..final_count {
            let mut attributes = HashMap::new();
            attributes.insert("Norm".to_string(), GAttribute::Float3([0.0, 1.0, 0.0]));
            attributes.insert("UV".to_string(), GAttribute::Float2([0.0, 0.0]));
            geom.vertices.push(GVertex {
                pos: [bufs.out_pos[i * 3], bufs.out_pos[i * 3 + 1], bufs.out_pos[i * 3 + 2]],
                col: [bufs.out_col[i * 3], bufs.out_col[i * 3 + 1], bufs.out_col[i * 3 + 2]],
                attributes,
            });
        }
    } else {
        let binds: Vec<Val> = vec![
            Val::P(PtrV::Buf(BufId::RwPos)),
            Val::P(PtrV::Buf(BufId::RwCol)),
            Val::I(count as i64),
            Val::P(PtrV::Buf(BufId::Params)),
        ];
        let n_bind = if arity >= 4 { 4 } else { 3.min(arity) };

        let mut bufs = Bufs {
            in_pos: &[],
            in_col: &[],
            out_pos: Vec::new(),
            out_col: Vec::new(),
            out_count: 0,
            rw_pos: in_pos.clone(),
            rw_col: in_col.clone(),
            params: &param_data,
            attrs: Vec::new(),
        };

        let mut steps = 0u64;
        for id in 0..count {
            let mut interp = Interp {
                fns: &fns,
                slots: Vec::new(),
                scopes: vec![HashMap::new()],
                bufs: std::mem::replace(
                    &mut bufs,
                    Bufs { in_pos: &[], in_col: &[], out_pos: Vec::new(), out_col: Vec::new(), out_count: 0, rw_pos: Vec::new(), rw_col: Vec::new(), params: &[], attrs: Vec::new() },
                ),
                global_id: id,
                steps,
                budget,
            };
            for (i, (name, is_ptr)) in pnames.iter().enumerate().take(n_bind) {
                let data = match binds[i] {
                    Val::P(p) if *is_ptr => SlotData::Ptr(p),
                    Val::I(n) => SlotData::I(n),
                    Val::F(f) => SlotData::F(f),
                    Val::P(p) => SlotData::Ptr(p),
                };
                interp.declare(name, data);
            }
            interp.run_block(&process.body)?;
            steps = interp.steps;
            bufs = interp.bufs;
        }

        for (i, v) in geom.vertices.iter_mut().enumerate() {
            v.pos = [bufs.rw_pos[i * 3], bufs.rw_pos[i * 3 + 1], bufs.rw_pos[i * 3 + 2]];
            v.col = [bufs.rw_col[i * 3], bufs.rw_col[i * 3 + 1], bufs.rw_col[i * 3 + 2]];
        }
    }
    Ok(())
}

/// Force the CPU backend regardless of OpenCL availability (`CCE_KERNEL_CPU=1`).
pub fn forced() -> bool {
    std::env::var("CCE_KERNEL_CPU").is_ok_and(|v| v != "0" && !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(code: &str, params: &[f32]) -> Geometry {
        let mut g = Geometry::new();
        run_kernel_cpu(code, &mut g, params).expect("kernel runs");
        g
    }

    /// The C corners a naive evaluator gets wrong, in one kernel: postfix ++
    /// yielding the OLD value, int division truncating, casts, ternary,
    /// short-circuit &&, and %.
    #[test]
    fn c_semantics() {
        let code = r#"
            __kernel void process(__global const float* in_pos, __global const float* in_col, int in_count,
                                  __global float* out_pos, __global float* out_col, __global int* out_count, int max_vertices) {
                int id = get_global_id(0);
                if (id == 0) {
                    int count = 0;
                    int idx = count++;                 // idx = 0, count = 1
                    int div = 7 / 2;                   // 3, not 3.5
                    float fdiv = (float)7 / 2.0f;      // 3.5
                    int rem = 7 % 4;                   // 3
                    int t = div == 3 ? 10 : 20;        // 10
                    int guard = 0;
                    if (guard && (1 / guard) > 0) { t = 999; }   // must not divide
                    out_pos[0] = (float)idx; out_pos[1] = (float)count; out_pos[2] = (float)div;
                    out_col[0] = fdiv; out_col[1] = (float)rem; out_col[2] = (float)t;
                    *out_count = 1;
                }
            }"#;
        let g = gen(code, &[]);
        assert_eq!(g.vertices.len(), 1);
        assert_eq!(g.vertices[0].pos, [0.0, 1.0, 3.0]);
        assert_eq!(g.vertices[0].col, [3.5, 3.0, 10.0]);
    }

    /// User-defined function with buffer pointers and &address-of an int —
    /// the box template's add_box shape.
    #[test]
    fn user_function_with_pointers() {
        let code = r#"
            void emit(float x, __global float* out_pos, __global float* out_col, int* count, int max_vertices) {
                int start = *count;
                if (start < max_vertices) {
                    out_pos[start * 3 + 0] = x;
                    out_pos[start * 3 + 1] = x * 2.0f;
                    out_pos[start * 3 + 2] = 0.0f;
                    out_col[start * 3 + 0] = 1.0f;
                }
                *count = start + 1;
            }
            __kernel void process(__global const float* in_pos, __global const float* in_col, int in_count,
                                  __global float* out_pos, __global float* out_col, __global int* out_count, int max_vertices) {
                if (get_global_id(0) == 0) {
                    int count = 0;
                    for (int i = 0; i < 3; i++) {
                        emit((float)i + 1.0f, out_pos, out_col, &count, max_vertices);
                    }
                    *out_count = count;
                }
            }"#;
        let g = gen(code, &[]);
        assert_eq!(g.vertices.len(), 3);
        assert_eq!(g.vertices[1].pos, [2.0, 4.0, 0.0]);
        assert_eq!(g.vertices[2].pos, [3.0, 6.0, 0.0]);
    }

    /// Local fixed arrays with initializer lists — the sphere/box pattern.
    #[test]
    fn array_initializers() {
        let code = r#"
            __kernel void process(__global const float* in_pos, __global const float* in_col, int in_count,
                                  __global float* out_pos, __global float* out_col, __global int* out_count, int max_vertices) {
                if (get_global_id(0) == 0) {
                    float px[3] = {1.5f, 2.5f, 3.5f};
                    int order[3] = {2, 0, 1};
                    int n = 0;
                    for (int i = 0; i < 3; i++) {
                        int idx = n++;
                        out_pos[idx * 3] = px[order[i]];
                    }
                    *out_count = n;
                }
            }"#;
        let g = gen(code, &[]);
        assert_eq!(g.vertices.len(), 3);
        assert_eq!(g.vertices[0].pos[0], 3.5);
        assert_eq!(g.vertices[1].pos[0], 1.5);
        assert_eq!(g.vertices[2].pos[0], 2.5);
    }

    /// Deformer mode: per-vertex ids, in-place pos/col, param_values binding.
    #[test]
    fn deformer_mode_and_params() {
        let code = r#"
            __kernel void process(__global float* pos, __global float* col, int count, __global const float* param_values) {
                int id = get_global_id(0);
                if (id < count) {
                    pos[id * 3 + 1] = pos[id * 3 + 1] + param_values[0];
                }
            }"#;
        let mut g = Geometry::new();
        for i in 0..4 {
            g.vertices.push(GVertex {
                pos: [i as f32, 1.0, 0.0],
                col: [0.0; 3],
                attributes: HashMap::new(),
            });
        }
        run_kernel_cpu(code, &mut g, &[2.5]).unwrap();
        for v in &g.vertices {
            assert_eq!(v.pos[1], 3.5);
        }
    }

    /// A kernel that never terminates errors out instead of hanging the app.
    #[test]
    fn step_budget_stops_runaway_kernels() {
        let code = r#"
            __kernel void process(__global const float* in_pos, __global const float* in_col, int in_count,
                                  __global float* out_pos, __global float* out_col, __global int* out_count, int max_vertices) {
                while (1) { int x = 0; }
            }"#;
        let mut g = Geometry::new();
        let err = run_kernel_cpu_with_budget(code, &mut g, &[], 100_000).unwrap_err();
        assert!(err.contains("step budget"), "unexpected error: {err}");
    }
}

/// Template-kernel validation: the CPU backend run against the REAL shipped
/// kernels, with absolute geometric asserts (green with or without an OpenCL
/// runtime — the coverage the OpenCL-side tests could never give headless),
/// and a cross-validation pass comparing CPU output to OpenCL vertex-by-vertex
/// whenever a platform exists. The reference is only a reference if the two
/// backends agree.
#[cfg(test)]
mod template_tests {
    use super::*;
    use crate::geometry::{parse_dynamic_params, preprocess_opencl_code, run_opencl_kernel_with_params};

    /// A template's kernel Code + the default param values, exactly as the
    /// resolve path would flatten them.
    fn template_kernel(template_name: &str) -> (String, Vec<f32>) {
        let root = crate::app::load_fs_tree();
        let tpl = root
            .children
            .iter()
            .find(|t| t.name == template_name)
            .unwrap_or_else(|| panic!("{template_name} template"));
        let opencl = tpl
            .children
            .iter()
            .find(|c| c.node_type == "opencl")
            .expect("template has an opencl child");
        let code = opencl
            .params
            .iter()
            .find(|p| p.name == "Code")
            .expect("Code param")
            .default
            .clone();
        let mut flat = Vec::new();
        for p in parse_dynamic_params(&code) {
            if p.param_type == "float3" {
                let parts: Vec<f32> = p.default.split(':').filter_map(|s| s.parse().ok()).collect();
                flat.extend_from_slice(&[
                    parts.first().copied().unwrap_or(0.0),
                    parts.get(1).copied().unwrap_or(0.0),
                    parts.get(2).copied().unwrap_or(0.0),
                ]);
            } else if p.default.eq_ignore_ascii_case("true") {
                flat.push(1.0);
            } else if p.default.eq_ignore_ascii_case("false") {
                flat.push(0.0);
            } else {
                flat.push(p.default.parse().unwrap_or(0.0));
            }
        }
        (preprocess_opencl_code(&code), flat)
    }

    fn triangle() -> Geometry {
        let mut g = Geometry::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            g.vertices.push(GVertex { pos: p, col: [0.5; 3], attributes: HashMap::new() });
        }
        g
    }

    #[test]
    fn cpu_runs_the_sphere_template() {
        let (code, params) = template_kernel("Sphere");
        let mut g = Geometry::new();
        run_kernel_cpu(&code, &mut g, &params).expect("sphere kernel");
        // Same asserts as the OpenCL-side test: 16*24*6 vertices on a 0.5
        // sphere centred at (0, 0.55, 0).
        assert_eq!(g.vertices.len(), 2304);
        let max_dist = g
            .vertices
            .iter()
            .map(|v| {
                let (dx, dy, dz) = (v.pos[0], v.pos[1] - 0.55, v.pos[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .fold(0.0f32, f32::max);
        assert!((max_dist - 0.5).abs() < 0.01, "radius {max_dist}");
    }

    #[test]
    fn cpu_runs_the_plane_box_and_extrude_templates() {
        for (name, input, expect_nonempty) in [
            ("Plane", Geometry::new(), true),
            ("Box", Geometry::new(), true),
            ("Extrude", triangle(), true),
        ] {
            let (code, params) = template_kernel(name);
            let mut g = input;
            run_kernel_cpu(&code, &mut g, &params).unwrap_or_else(|e| panic!("{name} kernel: {e}"));
            assert_eq!(!g.vertices.is_empty(), expect_nonempty, "{name} produced no geometry");
            for v in &g.vertices {
                assert!(v.pos.iter().all(|c| c.is_finite()), "{name} produced non-finite positions");
            }
        }
    }

    /// The reference test proper: byte-level agreement with OpenCL on every
    /// shipped kernel. Skips silently where no platform exists — the absolute
    /// tests above still cover the CPU side there.
    #[test]
    fn cpu_matches_opencl_on_every_shipped_kernel() {
        for (name, input) in [
            ("Sphere", Geometry::new()),
            ("Plane", Geometry::new()),
            ("Box", Geometry::new()),
            ("Extrude", triangle()),
        ] {
            let (code, params) = template_kernel(name);

            let mut gpu = input.clone();
            match run_opencl_kernel_with_params(&code, &mut gpu, &params) {
                Err(e) if e.contains("No OpenCL platforms") => return, // headless: nothing to compare against
                Err(e) => panic!("{name} on OpenCL: {e}"),
                Ok(()) => {}
            }

            let mut cpu = input;
            run_kernel_cpu(&code, &mut cpu, &params).unwrap_or_else(|e| panic!("{name} on CPU: {e}"));

            assert_eq!(cpu.vertices.len(), gpu.vertices.len(), "{name}: vertex count diverges");
            for (i, (a, b)) in cpu.vertices.iter().zip(&gpu.vertices).enumerate() {
                for k in 0..3 {
                    assert!(
                        (a.pos[k] - b.pos[k]).abs() < 1e-4,
                        "{name}: pos[{k}] diverges at vertex {i}: cpu {} vs gpu {}",
                        a.pos[k],
                        b.pos[k]
                    );
                    assert!(
                        (a.col[k] - b.col[k]).abs() < 1e-4,
                        "{name}: col[{k}] diverges at vertex {i}: cpu {} vs gpu {}",
                        a.col[k],
                        b.col[k]
                    );
                }
            }
        }
    }
}
