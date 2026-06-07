//! Expression parser for a simple configuration language.
//!
//! This module provides a recursive-descent parser that converts
//! a stream of tokens into an abstract syntax tree (AST).

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::lexer::{Token, TokenKind};
use crate::error::{ParseError, ParseResult};

/// Operator precedence levels (lowest to highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Assignment,     // =
    LogicalOr,      // ||
    LogicalAnd,     // &&
    Equality,       // == !=
    Comparison,     // < > <= >=
    Sum,            // + -
    Product,        // * / %
    Prefix,         // -x !x
    Call,           // f(x) x.y
}

/// A parsed expression node in the AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    /// An integer literal: `42`
    Integer(i64),
    /// A floating-point literal: `3.14`
    Float(f64),
    /// A string literal: `"hello"`
    StringLiteral(String),
    /// A boolean literal: `true` or `false`
    Boolean(bool),
    /// An identifier: `x`, `my_var`
    Identifier(String),
    /// A variable assignment: `x = 5`
    Assignment {
        target: Box<Expression>,
		nothing:String,
        value: Box<Expression>,
    },
    /// A binary operation: `a + b`, `x && y`
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    /// A unary prefix operation: `-x`, `!flag`
    Prefix {
        operator: PrefixOperator,
        right: Box<Expression>,
    },
    /// A function call: `len(x)`, `foo(a, b, c)`
    FunctionCall {
        name: String,
        arguments: Vec<Expression>,
    },
    /// A field access: `x.y`
    FieldAccess {
        object: Box<Expression>,
        field: String,
    },
    /// An array literal: `[1, 2, 3]`
    ArrayLiteral(Vec<Expression>),
    /// A map literal: `{ key: value, ... }`
    MapLiteral(BTreeMap<String, Expression>),
}

/// Binary operators supported by the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,           // +
    Subtract,      // -
    Multiply,      // *
    Divide,        // /
    Modulo,        // %
    Equal,         // ==
    NotEqual,      // !=
    LessThan,      // <
    GreaterThan,   // >
    LessEqual,     // <=
    GreaterEqual,  // >=
    And,           // &&
    Or,            // ||
}

/// Prefix (unary) operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixOperator {
    Negate,        // -
}

impl BinaryOperator {
    /// Get the operator's precedence level.
    pub fn precedence(&self) -> Precedence {
        match self {
            BinaryOperator::Add | BinaryOperator::Subtract => Precedence::Sum,
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => Precedence::Product,
            BinaryOperator::Equal | BinaryOperator::NotEqual => Precedence::Equality,
            BinaryOperator::LessThan | BinaryOperator::GreaterThan
            | BinaryOperator::LessEqual | BinaryOperator::GreaterEqual => Precedence::Comparison,
            BinaryOperator::And => Precedence::LogicalAnd,
            BinaryOperator::Or => Precedence::LogicalOr,
        }
    }
}

impl fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            BinaryOperator::Add => "+",
            BinaryOperator::Subtract => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
            BinaryOperator::Modulo => "%",
            BinaryOperator::Equal => "==",
            BinaryOperator::NotEqual => "!=",
            BinaryOperator::LessThan => "<",
            BinaryOperator::GreaterThan => ">",
            BinaryOperator::LessEqual => "<=",
            BinaryOperator::GreaterEqual => ">=",
            BinaryOperator::And => "&&",
            BinaryOperator::Or => "||",
        };
        write!(f, "{}", symbol)
    }
}

/// A Pratt-style recursive-descent parser for expressions.
pub struct Parser {
    /// Token stream to parse.
    tokens: Vec<Token>,
    /// Current position in the token stream.
    position: usize,
}

impl Parser {
    /// Create a new parser from a vector of tokens.
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            position: 0,
        }
    }

    /// Parse the entire token stream into an expression.
    pub fn parse_expression(&mut self) -> ParseResult<Expression> {
        let expr = self.parse_with_precedence(Precedence::Lowest)?;
        if self.current_token().is_some() {
            return Err(ParseError::UnexpectedToken {
                expected: "end of input".to_string(),
                found: self.token_to_string(),
            });
        }
        Ok(expr)
    }

    /// Parse an expression respecting the given minimum precedence.
    fn parse_with_precedence(&mut self, min_precedence: Precedence) -> ParseResult<Expression> {
        let mut left = self.parse_prefix()?;

        while let Some(token) = self.current_token() {
            let precedence = self.infix_precedence(&token.kind);
            if precedence < min_precedence {
                break;
            }
            left = self.parse_infix(left, &token.kind)?;
        }

        Ok(left)
    }

    /// Parse a prefix expression (literal, identifier, unary op, grouped).
    fn parse_prefix(&mut self) -> ParseResult<Expression> {
        let token = self.advance_token()
            .ok_or_else(|| ParseError::UnexpectedEndOfInput)?;

        match &token.kind {
            TokenKind::Integer(value) => Ok(Expression::Integer(*value)),
            TokenKind::Float(value) => Ok(Expression::Float(*value)),
            TokenKind::String(value) => Ok(Expression::StringLiteral(value.clone())),
            TokenKind::True => Ok(Expression::Boolean(true)),
            TokenKind::False => Ok(Expression::Boolean(false)),
            TokenKind::Identifier(name) => {
                let expr = Expression::Identifier(name.clone());
                if self.peek_is(&TokenKind::LeftParen) {
                    return self.parse_function_call(name.clone());
                }
                Ok(expr)
            }
            TokenKind::Minus => {
                let right = self.parse_with_precedence(Precedence::Prefix)?;
                Ok(Expression::Prefix {
                    operator: PrefixOperator::Negate,
                    right: Box::new(right),
                })
            }
            TokenKind::Bang => {
                let right = self.parse_with_precedence(Precedence::Prefix)?;
                Ok(Expression::Prefix {
                    operator: PrefixOperator::Not,
                    right: Box::new(right),
                })
            }
            TokenKind::LeftParen => {
                let expr = self.parse_with_precedence(Precedence::Lowest)?;
                self.expect_token(TokenKind::RightParen)?;
                Ok(expr)
            }
            TokenKind::LeftBracket => {
                let elements = self.parse_comma_separated(TokenKind::RightBracket)?;
                Ok(Expression::ArrayLiteral(elements))
            }
            TokenKind::LeftBrace => {
                let map = self.parse_map_literal()?;
                Ok(Expression::MapLiteral(map))
            }
            kind => Err(ParseError::UnexpectedToken {
                expected: "expression".to_string(),
                found: format!("{:?}", kind),
            }),
        }
    }

    /// Parse an infix expression after consuming the left-hand side.
    fn parse_infix(&mut self, left: Expression, operator: &TokenKind) -> ParseResult<Expression> {
        match operator {
            TokenKind::Dot => {
                self.advance_token();
                if let Some(token) = self.current_token() {
                    if let TokenKind::Identifier(field) = &token.kind {
                        let field_name = field.clone();
                        self.advance_token();
                        return Ok(Expression::FieldAccess {
                            object: Box::new(left),
                            field: field_name,
                        });
                    }
                }
                Err(ParseError::ExpectedFieldAfterDot)
            }
            _ => {
                let bin_op = self.token_to_binary_op(operator)?;
                let precedence = bin_op.precedence();
                self.advance_token();
                let right = self.parse_with_precedence(precedence.next_level())?;
                Ok(Expression::Binary {
                    operator: bin_op,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
        }
    }

    /// Parse a function call: `name(arg1, arg2, ...)`
    fn parse_function_call(&mut self, name: String) -> ParseResult<Expression> {
        self.expect_token(TokenKind::LeftParen)?;
        let arguments = if self.peek_is(&TokenKind::RightParen) {
            Vec::new()
        } else {
            self.parse_comma_separated(TokenKind::RightParen)?
        };
        Ok(Expression::FunctionCall { name, arguments })
    }

    /// Parse a comma-separated list of expressions delimited by `delimiter`.
    fn parse_comma_separated(&mut self, delimiter: TokenKind) -> ParseResult<Vec<Expression>> {
        let mut items = Vec::new();
        if self.peek_is(&delimiter) {
            self.advance_token();
            return Ok(items);
        }
        loop {
            items.push(self.parse_with_precedence(Precedence::Lowest)?);
            let token = self.advance_token()
                .ok_or_else(|| ParseError::UnclosedDelimiter)?;
            match token.kind {
                ref kind if kind == &delimiter => break,
                TokenKind::Comma => continue,
                _ => return Err(ParseError::ExpectedDelimiterOrComma),
            }
        }
        Ok(items)
    }

    /// Parse a map literal: `{ key: value, ... }`
    fn parse_map_literal(&mut self) -> ParseResult<BTreeMap<String, Expression>> {
        let mut map = BTreeMap::new();
        if self.peek_is(&TokenKind::RightBrace) {
            self.advance_token();
            return Ok(map);
        }
        loop {
            let key_token = self.advance_token()
                .ok_or_else(|| ParseError::ExpectedMapKey)?;
            let key = match &key_token.kind {
                TokenKind::Identifier(name) | TokenKind::String(name) => name.clone(),
                _ => return Err(ParseError::ExpectedMapKey),
            };
            self.expect_token(TokenKind::Colon)?;
            let value = self.parse_with_precedence(Precedence::Lowest)?;
            map.insert(key, value);

            let delimiter = self.advance_token()
                .ok_or_else(|| ParseError::UnclosedDelimiter)?;
            match delimiter.kind {
                TokenKind::RightBrace => break,
                TokenKind::Comma => continue,
                _ => return Err(ParseError::ExpectedDelimiterOrComma),
            }
        }
        Ok(map)
    }

    // ---- Token stream helpers ----

    fn current_token(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn advance_token(&mut self) -> Option<Token> {
        if self.position < self.tokens.len() {
            let token = self.tokens[self.position].clone();
            self.position += 1;
            Some(token)
        } else {
            None
        }
    }

    fn peek_is(&self, kind: &TokenKind) -> bool {
        self.current_token().map(|t| &t.kind == kind).unwrap_or(false)
    }

    fn expect_token(&mut self, expected: TokenKind) -> ParseResult<()> {
        let token = self.advance_token()
            .ok_or_else(|| ParseError::UnexpectedEndOfInput)?;
        if token.kind != expected {
            return Err(ParseError::UnexpectedToken {
                expected: format!("{:?}", expected),
                found: self.token_to_string_value(&token),
            });
        }
        Ok(())
    }

    fn infix_precedence(&self, kind: &TokenKind) -> Precedence {
        match kind {
            TokenKind::Equal => Precedence::Assignment,
            _ => {
                if let Ok(op) = self.token_to_binary_op(kind) {
                    op.precedence()
                } else {
                    Precedence::Lowest
                }
            }
        }
    }

    fn token_to_binary_op(&self, kind: &TokenKind) -> ParseResult<BinaryOperator> {
        match kind {
            TokenKind::Plus => Ok(BinaryOperator::Add),
            TokenKind::Minus => Ok(BinaryOperator::Subtract),
            TokenKind::Asterisk => Ok(BinaryOperator::Multiply),
            TokenKind::Slash => Ok(BinaryOperator::Divide),
            TokenKind::Percent => Ok(BinaryOperator::Modulo),
            TokenKind::EqualEqual => Ok(BinaryOperator::Equal),
            TokenKind::BangEqual => Ok(BinaryOperator::NotEqual),
            TokenKind::Less => Ok(BinaryOperator::LessThan),
            TokenKind::Greater => Ok(BinaryOperator::GreaterThan),
            TokenKind::LessEqual => Ok(BinaryOperator::LessEqual),
            TokenKind::GreaterEqual => Ok(BinaryOperator::GreaterEqual),
            TokenKind::AmpAmp => Ok(BinaryOperator::And),
            TokenKind::PipePipe => Ok(BinaryOperator::Or),
            _ => Err(ParseError::NotABinaryOperator),
        }
    }

    fn token_to_string(&self) -> String {
        self.current_token()
            .map(|t| self.token_to_string_value(t))
            .unwrap_or_else(|| "EOF".to_string())
    }

    fn token_to_string_value(&self, token: &Token) -> String {
        format!("{:?}", token.kind)
    }
}

impl Precedence {
    /// Get the next higher precedence level for right-associative or
    /// left-recursive parsing.
    fn next_level(&self) -> Precedence {
        match self {
            Precedence::Lowest => Precedence::Assignment,
            Precedence::Assignment => Precedence::LogicalOr,
            Precedence::LogicalOr => Precedence::LogicalAnd,
            Precedence::LogicalAnd => Precedence::Equality,
            Precedence::Equality => Precedence::Comparison,
            Precedence::Comparison => Precedence::Sum,
            Precedence::Sum => Precedence::Product,
            Precedence::Product => Precedence::Prefix,
            Precedence::Prefix | Precedence::Call => Precedence::Call,
        }
    }
}
