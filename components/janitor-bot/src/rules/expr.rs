use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::I64(n) => write!(f, "{n}"),
            Value::F64(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "\"{s}\""),
        }
    }
}

impl Value {
    pub fn as_bool(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            other => Err(format!("expected bool, got {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Var(String),
    Lit(Value),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cmp(Box<Expr>, CmpOp, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    True,
    False,
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Ne);
                } else {
                    tokens.push(Token::Not);
                }
            }
            '&' => {
                chars.next();
                if chars.next() != Some('&') {
                    return Err("expected '&&'".to_string());
                }
                tokens.push(Token::And);
            }
            '|' => {
                chars.next();
                if chars.next() != Some('|') {
                    return Err("expected '||'".to_string());
                }
                tokens.push(Token::Or);
            }
            '=' => {
                chars.next();
                if chars.next() != Some('=') {
                    return Err("expected '=='".to_string());
                }
                tokens.push(Token::Eq);
            }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Le);
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Ge);
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '"' | '\'' => {
                let quote = c;
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some(ch) if ch == quote => break,
                        Some(ch) => s.push(ch),
                        None => return Err("unterminated string literal".to_string()),
                    }
                }
                tokens.push(Token::Str(s));
            }
            _ if c.is_ascii_digit()
                || (c == '-' && matches!(chars.clone().nth(1), Some(d) if d.is_ascii_digit())) =>
            {
                let mut num = String::new();
                if c == '-' {
                    num.push('-');
                    chars.next();
                }
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() || ch == '.' {
                        num.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if num.contains('.') {
                    let f: f64 = num.parse().map_err(|e| format!("invalid float: {e}"))?;
                    tokens.push(Token::Float(f));
                } else {
                    let i: i64 = num.parse().map_err(|e| format!("invalid integer: {e}"))?;
                    tokens.push(Token::Int(i));
                }
            }
            _ if c.is_alphanumeric() || c == '_' => {
                let mut ident = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        ident.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match ident.as_str() {
                    "true" => tokens.push(Token::True),
                    "false" => tokens.push(Token::False),
                    _ => tokens.push(Token::Ident(ident)),
                }
            }
            _ => return Err(format!("unexpected character: '{c}'")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.next();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        while self.peek() == Some(&Token::And) {
            self.next();
            let right = self.parse_comparison()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let left = self.parse_unary()?;
        let op = match self.peek() {
            Some(Token::Eq) => CmpOp::Eq,
            Some(Token::Ne) => CmpOp::Ne,
            Some(Token::Lt) => CmpOp::Lt,
            Some(Token::Le) => CmpOp::Le,
            Some(Token::Gt) => CmpOp::Gt,
            Some(Token::Ge) => CmpOp::Ge,
            _ => return Ok(left),
        };
        self.next();
        let right = self.parse_unary()?;
        Ok(Expr::Cmp(Box::new(left), op, Box::new(right)))
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::Not) {
            self.next();
            let expr = self.parse_unary()?;
            return Ok(Expr::Not(Box::new(expr)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                if self.next() != Some(Token::RParen) {
                    return Err("expected ')'".to_string());
                }
                Ok(expr)
            }
            Some(Token::True) => Ok(Expr::Lit(Value::Bool(true))),
            Some(Token::False) => Ok(Expr::Lit(Value::Bool(false))),
            Some(Token::Int(n)) => Ok(Expr::Lit(Value::I64(n))),
            Some(Token::Float(n)) => Ok(Expr::Lit(Value::F64(n))),
            Some(Token::Str(s)) => Ok(Expr::Lit(Value::String(s))),
            Some(Token::Ident(name)) => Ok(Expr::Var(name)),
            Some(tok) => Err(format!("unexpected token: {tok:?}")),
            None => Err("unexpected end of expression".to_string()),
        }
    }
}

pub fn parse(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(Expr::Lit(Value::Bool(true)));
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!("unexpected token at position {}", parser.pos));
    }
    Ok(expr)
}

fn cmp_values(lhs: &Value, op: CmpOp, rhs: &Value) -> Result<bool, String> {
    match (lhs, rhs) {
        (Value::I64(a), Value::I64(b)) => Ok(match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }),
        (Value::F64(a), Value::F64(b)) => Ok(match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }),
        (Value::I64(a), Value::F64(b)) => {
            let a = *a as f64;
            Ok(match op {
                CmpOp::Eq => a == *b,
                CmpOp::Ne => a != *b,
                CmpOp::Lt => a < *b,
                CmpOp::Le => a <= *b,
                CmpOp::Gt => a > *b,
                CmpOp::Ge => a >= *b,
            })
        }
        (Value::F64(_), Value::I64(_)) => cmp_values(rhs, op.flip(), lhs),
        (Value::String(a), Value::String(b)) => Ok(match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }),
        (Value::Bool(a), Value::Bool(b)) => match op {
            CmpOp::Eq => Ok(a == b),
            CmpOp::Ne => Ok(a != b),
            _ => Err(format!("cannot compare bools with {op:?}")),
        },
        _ => Err(format!("cannot compare {lhs} with {rhs}")),
    }
}

impl CmpOp {
    fn flip(self) -> Self {
        match self {
            CmpOp::Eq => CmpOp::Eq,
            CmpOp::Ne => CmpOp::Ne,
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
        }
    }
}

pub fn eval(expr: &Expr, vars: &HashMap<String, Value>) -> Result<Value, String> {
    match expr {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Var(name) => vars
            .get(name)
            .cloned()
            .ok_or_else(|| format!("undefined variable: '{name}'")),
        Expr::Not(e) => {
            let v = eval(e, vars)?;
            Ok(Value::Bool(!v.as_bool()?))
        }
        Expr::And(a, b) => {
            let lhs = eval(a, vars)?.as_bool()?;
            if !lhs {
                return Ok(Value::Bool(false));
            }
            let rhs = eval(b, vars)?.as_bool()?;
            Ok(Value::Bool(rhs))
        }
        Expr::Or(a, b) => {
            let lhs = eval(a, vars)?.as_bool()?;
            if lhs {
                return Ok(Value::Bool(true));
            }
            let rhs = eval(b, vars)?.as_bool()?;
            Ok(Value::Bool(rhs))
        }
        Expr::Cmp(lhs, op, rhs) => {
            let l = eval(lhs, vars)?;
            let r = eval(rhs, vars)?;
            Ok(Value::Bool(cmp_values(&l, *op, &r)?))
        }
    }
}

pub fn referenced_vars(expr: &Expr) -> Vec<String> {
    let mut vars = Vec::new();
    collect_vars(expr, &mut vars);
    vars
}

fn collect_vars(expr: &Expr, vars: &mut Vec<String>) {
    match expr {
        Expr::Var(name) => vars.push(name.clone()),
        Expr::Lit(_) => {}
        Expr::Not(e) => collect_vars(e, vars),
        Expr::And(a, b) | Expr::Or(a, b) => {
            collect_vars(a, vars);
            collect_vars(b, vars);
        }
        Expr::Cmp(a, _, b) => {
            collect_vars(a, vars);
            collect_vars(b, vars);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn bool_vars(pairs: &[(&str, bool)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::Bool(*v)))
            .collect()
    }

    fn eval_bool(input: &str, v: &HashMap<String, Value>) -> bool {
        eval(&parse(input).unwrap(), v).unwrap().as_bool().unwrap()
    }

    #[test]
    fn simple_var() {
        assert!(eval_bool("foo", &bool_vars(&[("foo", true)])));
        assert!(!eval_bool("foo", &bool_vars(&[("foo", false)])));
    }

    #[test]
    fn not() {
        assert!(!eval_bool("!foo", &bool_vars(&[("foo", true)])));
        assert!(eval_bool("!foo", &bool_vars(&[("foo", false)])));
    }

    #[test]
    fn and_or() {
        let v = bool_vars(&[("a", true), ("b", true), ("c", false)]);
        assert!(eval_bool("a && b || c", &v));
        let v = bool_vars(&[("a", false), ("b", false), ("c", true)]);
        assert!(eval_bool("a && b || c", &v));
        let v = bool_vars(&[("a", true), ("b", false), ("c", false)]);
        assert!(!eval_bool("a && b || c", &v));
    }

    #[test]
    fn parens() {
        let v = bool_vars(&[("a", true), ("b", false), ("c", true)]);
        assert!(eval_bool("a && (b || c)", &v));
        let v = bool_vars(&[("a", false), ("b", true), ("c", true)]);
        assert!(!eval_bool("a && (b || c)", &v));
    }

    #[test]
    fn empty_is_true() {
        assert!(eval_bool("", &HashMap::new()));
    }

    #[test]
    fn literals() {
        assert!(eval_bool("true && !false", &HashMap::new()));
    }

    #[test]
    fn undefined_var_errors() {
        let expr = parse("missing").unwrap();
        assert!(eval(&expr, &HashMap::new()).is_err());
    }

    #[test]
    fn referenced_vars_list() {
        let expr = parse("a && !b || c").unwrap();
        let mut refs = referenced_vars(&expr);
        refs.sort();
        assert_eq!(refs, vec!["a", "b", "c"]);
    }

    #[test]
    fn double_not() {
        assert!(eval_bool("!!a", &bool_vars(&[("a", true)])));
        assert!(!eval_bool("!!a", &bool_vars(&[("a", false)])));
    }

    #[test]
    fn nested_parens() {
        let v = bool_vars(&[("a", true), ("b", false), ("c", false), ("d", true)]);
        assert!(eval_bool("((a || b) && (c || d))", &v));
        let v = bool_vars(&[("a", false), ("b", false), ("c", true), ("d", true)]);
        assert!(!eval_bool("((a || b) && (c || d))", &v));
    }

    #[test]
    fn operator_precedence_and_binds_tighter() {
        let v = bool_vars(&[("a", true), ("b", false), ("c", false)]);
        assert!(eval_bool("a || b && c", &v));
        let v = bool_vars(&[("a", false), ("b", true), ("c", false)]);
        assert!(!eval_bool("a || b && c", &v));
        let v = bool_vars(&[("a", false), ("b", true), ("c", true)]);
        assert!(eval_bool("a || b && c", &v));
    }

    #[test]
    fn not_binds_tightest() {
        let v = bool_vars(&[("a", false), ("b", true)]);
        assert!(eval_bool("!a && b", &v));
        let v = bool_vars(&[("a", true), ("b", true)]);
        assert!(!eval_bool("!a && b", &v));
        let v = bool_vars(&[("a", false), ("b", false)]);
        assert!(!eval_bool("!a && b", &v));
    }

    #[test]
    fn complex_real_world_expression() {
        let v = bool_vars(&[
            ("in_window", true),
            ("needs_approval", true),
            ("already_queued", false),
        ]);
        assert!(eval_bool(
            "in_window && needs_approval && !already_queued",
            &v
        ));
        let v = bool_vars(&[
            ("in_window", true),
            ("needs_approval", true),
            ("already_queued", true),
        ]);
        assert!(!eval_bool(
            "in_window && needs_approval && !already_queued",
            &v
        ));
        let v = bool_vars(&[
            ("in_window", false),
            ("needs_approval", true),
            ("already_queued", false),
        ]);
        assert!(!eval_bool(
            "in_window && needs_approval && !already_queued",
            &v
        ));
    }

    #[test]
    fn whitespace_variations() {
        assert!(eval_bool(
            "  a   &&   b  ",
            &bool_vars(&[("a", true), ("b", true)])
        ));
    }

    #[test]
    fn single_ampersand_errors() {
        assert!(parse("a & b").is_err());
    }

    #[test]
    fn single_pipe_errors() {
        assert!(parse("a | b").is_err());
    }

    #[test]
    fn unclosed_paren_errors() {
        assert!(parse("(a && b").is_err());
    }

    #[test]
    fn unexpected_rparen_errors() {
        assert!(parse("a && b)").is_err());
    }

    #[test]
    fn empty_parens_errors() {
        assert!(parse("()").is_err());
    }

    #[test]
    fn trailing_operator_errors() {
        assert!(parse("a &&").is_err());
    }

    #[test]
    fn leading_operator_errors() {
        assert!(parse("&& a").is_err());
    }

    #[test]
    fn underscore_in_identifiers() {
        assert!(eval_bool(
            "my_var_1 && another_var_2",
            &bool_vars(&[("my_var_1", true), ("another_var_2", true)])
        ));
    }

    #[test]
    fn referenced_vars_with_literals() {
        let expr = parse("true && a || false && b").unwrap();
        let mut refs = referenced_vars(&expr);
        refs.sort();
        assert_eq!(refs, vec!["a", "b"]);
    }

    #[test]
    fn chained_or() {
        let v = bool_vars(&[("a", false), ("b", false), ("c", false), ("d", false)]);
        assert!(!eval_bool("a || b || c || d", &v));
        let v = bool_vars(&[("a", false), ("b", false), ("c", true), ("d", false)]);
        assert!(eval_bool("a || b || c || d", &v));
    }

    #[test]
    fn chained_and() {
        let v = bool_vars(&[("a", true), ("b", true), ("c", true), ("d", true)]);
        assert!(eval_bool("a && b && c && d", &v));
        let v = bool_vars(&[("a", true), ("b", true), ("c", false), ("d", true)]);
        assert!(!eval_bool("a && b && c && d", &v));
    }

    // comparison tests

    #[test]
    fn int_comparisons() {
        let v = vars(&[("x", Value::I64(5))]);
        assert!(eval_bool("x < 10", &v));
        assert!(!eval_bool("x < 5", &v));
        assert!(eval_bool("x <= 5", &v));
        assert!(eval_bool("x > 3", &v));
        assert!(!eval_bool("x > 5", &v));
        assert!(eval_bool("x >= 5", &v));
        assert!(eval_bool("x == 5", &v));
        assert!(!eval_bool("x == 6", &v));
        assert!(eval_bool("x != 6", &v));
    }

    #[test]
    fn float_comparisons() {
        let v = vars(&[("x", Value::F64(3.14))]);
        assert!(eval_bool("x < 4.0", &v));
        assert!(eval_bool("x > 3.0", &v));
        assert!(eval_bool("x == 3.14", &v));
    }

    #[test]
    fn int_float_mixed() {
        let v = vars(&[("x", Value::I64(5))]);
        assert!(eval_bool("x < 5.5", &v));
        assert!(!eval_bool("x > 5.5", &v));
    }

    #[test]
    fn string_comparisons() {
        let v = vars(&[("s", Value::String("hello".into()))]);
        assert!(eval_bool("s == \"hello\"", &v));
        assert!(!eval_bool("s == \"world\"", &v));
        assert!(eval_bool("s != \"world\"", &v));
    }

    #[test]
    fn string_single_quotes() {
        let v = vars(&[("s", Value::String("hello".into()))]);
        assert!(eval_bool("s == 'hello'", &v));
    }

    #[test]
    fn comparison_with_boolean_logic() {
        let v = vars(&[
            ("retry_count", Value::I64(2)),
            ("is_failure", Value::Bool(true)),
        ]);
        assert!(eval_bool("is_failure && retry_count < 3", &v));
        let v = vars(&[
            ("retry_count", Value::I64(3)),
            ("is_failure", Value::Bool(true)),
        ]);
        assert!(!eval_bool("is_failure && retry_count < 3", &v));
    }

    #[test]
    fn negative_int() {
        let v = vars(&[("x", Value::I64(-1))]);
        assert!(eval_bool("x < 0", &v));
        assert!(eval_bool("x == -1", &v));
    }

    #[test]
    fn bool_equality() {
        let v = bool_vars(&[("a", true)]);
        assert!(eval_bool("a == true", &v));
        assert!(!eval_bool("a == false", &v));
    }

    #[test]
    fn comparison_type_mismatch_errors() {
        let v = vars(&[("x", Value::I64(5))]);
        let expr = parse("x == \"hello\"").unwrap();
        assert!(eval(&expr, &v).is_err());
    }

    #[test]
    fn comparison_precedence() {
        // comparison binds tighter than && but looser than !
        let v = vars(&[("a", Value::Bool(true)), ("x", Value::I64(5))]);
        assert!(eval_bool("a && x < 10", &v));
        assert!(!eval_bool("a && x > 10", &v));
    }

    #[test]
    fn referenced_vars_in_comparisons() {
        let expr = parse("retry_count < 3 && is_failure").unwrap();
        let mut refs = referenced_vars(&expr);
        refs.sort();
        assert_eq!(refs, vec!["is_failure", "retry_count"]);
    }
}
