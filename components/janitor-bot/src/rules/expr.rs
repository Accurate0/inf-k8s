use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Expr {
    Var(String),
    Lit(bool),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Not,
    True,
    False,
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
                tokens.push(Token::Not);
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
        let mut left = self.parse_unary()?;
        while self.peek() == Some(&Token::And) {
            self.next();
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
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
            Some(Token::True) => Ok(Expr::Lit(true)),
            Some(Token::False) => Ok(Expr::Lit(false)),
            Some(Token::Ident(name)) => Ok(Expr::Var(name)),
            Some(tok) => Err(format!("unexpected token: {tok:?}")),
            None => Err("unexpected end of expression".to_string()),
        }
    }
}

pub fn parse(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(Expr::Lit(true));
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!("unexpected token at position {}", parser.pos));
    }
    Ok(expr)
}

pub fn eval(expr: &Expr, vars: &HashMap<String, bool>) -> Result<bool, String> {
    match expr {
        Expr::Lit(b) => Ok(*b),
        Expr::Var(name) => vars
            .get(name)
            .copied()
            .ok_or_else(|| format!("undefined variable: '{name}'")),
        Expr::Not(e) => Ok(!eval(e, vars)?),
        Expr::And(a, b) => Ok(eval(a, vars)? && eval(b, vars)?),
        Expr::Or(a, b) => Ok(eval(a, vars)? || eval(b, vars)?),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, bool)]) -> HashMap<String, bool> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn simple_var() {
        let expr = parse("foo").unwrap();
        assert!(eval(&expr, &vars(&[("foo", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("foo", false)])).unwrap());
    }

    #[test]
    fn not() {
        let expr = parse("!foo").unwrap();
        assert!(!eval(&expr, &vars(&[("foo", true)])).unwrap());
        assert!(eval(&expr, &vars(&[("foo", false)])).unwrap());
    }

    #[test]
    fn and_or() {
        let expr = parse("a && b || c").unwrap();
        assert!(eval(&expr, &vars(&[("a", true), ("b", true), ("c", false)])).unwrap());
        assert!(eval(&expr, &vars(&[("a", false), ("b", false), ("c", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", true), ("b", false), ("c", false)])).unwrap());
    }

    #[test]
    fn parens() {
        let expr = parse("a && (b || c)").unwrap();
        assert!(eval(&expr, &vars(&[("a", true), ("b", false), ("c", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", false), ("b", true), ("c", true)])).unwrap());
    }

    #[test]
    fn empty_is_true() {
        let expr = parse("").unwrap();
        assert!(eval(&expr, &HashMap::new()).unwrap());
    }

    #[test]
    fn literals() {
        let expr = parse("true && !false").unwrap();
        assert!(eval(&expr, &HashMap::new()).unwrap());
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
        let expr = parse("!!a").unwrap();
        assert!(eval(&expr, &vars(&[("a", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", false)])).unwrap());
    }

    #[test]
    fn nested_parens() {
        let expr = parse("((a || b) && (c || d))").unwrap();
        assert!(
            eval(
                &expr,
                &vars(&[("a", true), ("b", false), ("c", false), ("d", true)])
            )
            .unwrap()
        );
        assert!(
            !eval(
                &expr,
                &vars(&[("a", false), ("b", false), ("c", true), ("d", true)])
            )
            .unwrap()
        );
    }

    #[test]
    fn operator_precedence_and_binds_tighter() {
        // a || b && c should be a || (b && c)
        let expr = parse("a || b && c").unwrap();
        assert!(eval(&expr, &vars(&[("a", true), ("b", false), ("c", false)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", false), ("b", true), ("c", false)])).unwrap());
        assert!(eval(&expr, &vars(&[("a", false), ("b", true), ("c", true)])).unwrap());
    }

    #[test]
    fn not_binds_tightest() {
        // !a && b should be (!a) && b
        let expr = parse("!a && b").unwrap();
        assert!(eval(&expr, &vars(&[("a", false), ("b", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", true), ("b", true)])).unwrap());
        assert!(!eval(&expr, &vars(&[("a", false), ("b", false)])).unwrap());
    }

    #[test]
    fn complex_real_world_expression() {
        let expr = parse("in_window && needs_approval && !already_queued").unwrap();
        assert!(
            eval(
                &expr,
                &vars(&[
                    ("in_window", true),
                    ("needs_approval", true),
                    ("already_queued", false)
                ])
            )
            .unwrap()
        );
        assert!(
            !eval(
                &expr,
                &vars(&[
                    ("in_window", true),
                    ("needs_approval", true),
                    ("already_queued", true)
                ])
            )
            .unwrap()
        );
        assert!(
            !eval(
                &expr,
                &vars(&[
                    ("in_window", false),
                    ("needs_approval", true),
                    ("already_queued", false)
                ])
            )
            .unwrap()
        );
    }

    #[test]
    fn whitespace_variations() {
        let expr = parse("  a   &&   b  ").unwrap();
        assert!(eval(&expr, &vars(&[("a", true), ("b", true)])).unwrap());
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
        let expr = parse("my_var_1 && another_var_2").unwrap();
        assert!(eval(&expr, &vars(&[("my_var_1", true), ("another_var_2", true)])).unwrap());
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
        let expr = parse("a || b || c || d").unwrap();
        assert!(
            !eval(
                &expr,
                &vars(&[("a", false), ("b", false), ("c", false), ("d", false)])
            )
            .unwrap()
        );
        assert!(
            eval(
                &expr,
                &vars(&[("a", false), ("b", false), ("c", true), ("d", false)])
            )
            .unwrap()
        );
    }

    #[test]
    fn chained_and() {
        let expr = parse("a && b && c && d").unwrap();
        assert!(
            eval(
                &expr,
                &vars(&[("a", true), ("b", true), ("c", true), ("d", true)])
            )
            .unwrap()
        );
        assert!(
            !eval(
                &expr,
                &vars(&[("a", true), ("b", true), ("c", false), ("d", true)])
            )
            .unwrap()
        );
    }
}
