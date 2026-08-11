//! MQL parser: precedence-climbing recursive descent over the lexer output.

use std::time::{Duration, Instant};

use crate::ast::{CompareOp, MqlAst, MqlTerm, MqlValue};
use crate::errors::{MqlError, Span};
use crate::lexer::{Keyword, SpannedToken, Token, is_bool_keyword};
use crate::plan::Bounds;

pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
    deadline: Instant,
    timeout_ms: u64,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>, timeout_ms: u64) -> Self {
        Self {
            tokens,
            pos: 0,
            deadline: Instant::now() + Duration::from_millis(timeout_ms),
            timeout_ms,
        }
    }

    fn peek(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    fn peek_keyword(&self) -> Option<Keyword> {
        match self.peek() {
            Some((Token::Word(w), _)) => is_bool_keyword(w),
            _ => None,
        }
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some((Token::LParen, _)) => true,
            Some((Token::Minus, _)) => true,
            Some((Token::Quoted(_), _)) => true,
            Some((Token::Word(w), _)) => is_bool_keyword(w)
                .map(|k| k != Keyword::And && k != Keyword::Or)
                .unwrap_or(true),
            _ => false,
        }
    }

    fn bump(&mut self) -> SpannedToken {
        let t = self.tokens[self.pos].clone();
        self.pos += 1;
        t
    }

    pub fn parse(mut self, bounds: &Bounds) -> Result<MqlAst, MqlError> {
        if self.tokens.is_empty() {
            return Err(MqlError::EmptyInput);
        }
        let ast = self.parse_or(bounds, 1)?;
        if ast.leaf_count() > bounds.max_clauses {
            return Err(MqlError::ExceedsBound {
                kind: "clause count".to_string(),
                limit: bounds.max_clauses,
                span: None,
            });
        }
        if ast.depth() > bounds.max_ast_depth {
            return Err(MqlError::ExceedsBound {
                kind: "AST depth".to_string(),
                limit: bounds.max_ast_depth,
                span: None,
            });
        }
        if self.pos != self.tokens.len() {
            let (_, span) = self.tokens[self.pos];
            return Err(MqlError::Syntax {
                message: "unexpected trailing input".to_string(),
                span,
            });
        }
        Ok(ast)
    }

    fn parse_or(&mut self, bounds: &Bounds, depth: usize) -> Result<MqlAst, MqlError> {
        self.check_deadline()?;
        let mut left = self.parse_and(bounds, depth)?;
        while self.peek_keyword() == Some(Keyword::Or) {
            self.bump();
            let right = self.parse_and(bounds, depth)?;
            left = MqlAst::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self, bounds: &Bounds, depth: usize) -> Result<MqlAst, MqlError> {
        self.check_deadline()?;
        let mut left = self.parse_unary(bounds, depth)?;
        loop {
            match self.peek_keyword() {
                Some(Keyword::And) => {
                    self.bump();
                    let right = self.parse_unary(bounds, depth)?;
                    left = MqlAst::And(Box::new(left), Box::new(right));
                }
                _ => {
                    if self.starts_primary() {
                        let right = self.parse_unary(bounds, depth)?;
                        left = MqlAst::And(Box::new(left), Box::new(right));
                    } else {
                        break;
                    }
                }
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self, bounds: &Bounds, depth: usize) -> Result<MqlAst, MqlError> {
        self.check_deadline()?;
        self.check_depth(bounds, depth)?;
        if let Some(Keyword::Not) = self.peek_keyword() {
            self.bump();
            let inner = self.parse_unary(bounds, depth + 1)?;
            return Ok(MqlAst::Not(Box::new(inner)));
        }
        if let Some((Token::Minus, _)) = self.peek() {
            self.bump();
            let term = self.parse_primary(bounds, depth + 1)?;
            return match term {
                MqlAst::Term(mut t) => {
                    t.negate = true;
                    Ok(MqlAst::Term(t))
                }
                _ => Err(MqlError::Syntax {
                    message: "negation '-' must prefix a clause".to_string(),
                    span: self.span_of(&term),
                }),
            };
        }
        self.parse_primary(bounds, depth)
    }

    fn span_of(&self, ast: &MqlAst) -> Span {
        match ast {
            MqlAst::Term(t) => t.span,
            _ => (0, 0),
        }
    }

    fn parse_primary(&mut self, bounds: &Bounds, depth: usize) -> Result<MqlAst, MqlError> {
        self.check_deadline()?;
        self.check_depth(bounds, depth)?;
        if let Some((Token::LParen, _)) = self.peek() {
            self.bump();
            let inner = self.parse_or(bounds, depth + 1)?;
            match self.peek() {
                Some((Token::RParen, _)) => {
                    self.bump();
                    return Ok(inner);
                }
                Some((_, span)) => {
                    return Err(MqlError::Syntax {
                        message: "expected ')'".to_string(),
                        span: *span,
                    });
                }
                None => {
                    return Err(MqlError::Syntax {
                        message: "unexpected end of input, expected ')'".to_string(),
                        span: (0, 0),
                    });
                }
            }
        }
        self.parse_clause(bounds, depth)
    }

    fn parse_clause(&mut self, bounds: &Bounds, depth: usize) -> Result<MqlAst, MqlError> {
        self.check_deadline()?;
        let start_span = match self.peek() {
            Some((_, span)) => *span,
            None => {
                return Err(MqlError::Syntax {
                    message: "expected a clause".to_string(),
                    span: (0, 0),
                });
            }
        };
        let head = self.bump();
        let (field, op, value_span_start) = match head {
            (Token::Word(w), word_span) => {
                // Field clause if followed by Colon or a CompareOp.
                match self.peek() {
                    Some((Token::Colon, _)) => {
                        self.bump();
                        (Some(w), CompareOp::Eq, self.cur_span_start())
                    }
                    Some((Token::Compare(op), _)) => {
                        let op = *op;
                        self.bump();
                        (Some(w), op, self.cur_span_start())
                    }
                    _ => {
                        // Bare term.
                        let value = word_to_value(&w, word_span)?;
                        return Ok(MqlAst::Term(Box::new(MqlTerm {
                            field: None,
                            op: CompareOp::Eq,
                            value,
                            negate: false,
                            span: word_span,
                        })));
                    }
                }
            }
            (Token::Quoted(q), span) => {
                let value = MqlValue::Text(q);
                return Ok(MqlAst::Term(Box::new(MqlTerm {
                    field: None,
                    op: CompareOp::Eq,
                    value,
                    negate: false,
                    span,
                })));
            }
            (_, span) => {
                return Err(MqlError::Syntax {
                    message: "expected field or value".to_string(),
                    span,
                });
            }
        };

        let (value, end_span) = self.parse_value_with_range(value_span_start)?;
        let span = (start_span.0, end_span.1);
        if depth > bounds.max_ast_depth {
            return Err(MqlError::ExceedsBound {
                kind: "AST depth".to_string(),
                limit: bounds.max_ast_depth,
                span: Some(span),
            });
        }
        Ok(MqlAst::Term(Box::new(MqlTerm {
            field,
            op,
            value,
            negate: false,
            span,
        })))
    }

    fn cur_span_start(&self) -> Span {
        match self.peek() {
            Some((_, span)) => *span,
            None => (self.tokens.last().map(|t| t.1.1).unwrap_or(0), 0),
        }
    }

    fn parse_value_with_range(&mut self, _start: Span) -> Result<(MqlValue, Span), MqlError> {
        if matches!(self.peek(), Some((Token::Range, _))) {
            self.bump();
            let (high, end) = self.parse_single_value()?;
            return Ok((MqlValue::Range(None, Some(Box::new(high))), end));
        }
        let (low, end) = self.parse_single_value()?;
        if matches!(self.peek(), Some((Token::Range, _))) {
            self.bump();
            if matches!(self.peek(), Some((Token::Range, _))) || self.peek().is_none() {
                return Ok((MqlValue::Range(Some(Box::new(low)), None), end));
            }
            let (high, end2) = self.parse_single_value()?;
            return Ok((
                MqlValue::Range(Some(Box::new(low)), Some(Box::new(high))),
                end2,
            ));
        }
        Ok((low, end))
    }

    fn parse_single_value(&mut self) -> Result<(MqlValue, Span), MqlError> {
        self.check_deadline()?;
        let tok = match self.peek() {
            Some(t) => t.clone(),
            None => {
                return Err(MqlError::Syntax {
                    message: "expected a value".to_string(),
                    span: (0, 0),
                });
            }
        };
        // Advance past the token we just read.
        self.bump();
        match tok {
            (Token::Word(w), span) => {
                if w.len() >= 2 && w.starts_with('/') && w.ends_with('/') {
                    let inner = w[1..w.len() - 1].to_string();
                    Ok((MqlValue::Regex(inner), span))
                } else {
                    Ok((MqlValue::Text(w), span))
                }
            }
            (Token::Quoted(q), span) => Ok((MqlValue::Text(q), span)),
            (_, span) => Err(MqlError::Syntax {
                message: "expected a value".to_string(),
                span,
            }),
        }
    }

    fn check_deadline(&self) -> Result<(), MqlError> {
        if Instant::now() >= self.deadline {
            return Err(MqlError::ExceedsBound {
                kind: "parse timeout".to_string(),
                limit: usize::try_from(self.timeout_ms).unwrap_or(usize::MAX),
                span: self.peek().map(|token| token.1),
            });
        }
        Ok(())
    }

    fn check_depth(&self, bounds: &Bounds, depth: usize) -> Result<(), MqlError> {
        if depth > bounds.max_ast_depth {
            Err(MqlError::ExceedsBound {
                kind: "AST depth".to_string(),
                limit: bounds.max_ast_depth,
                span: self.peek().map(|token| token.1),
            })
        } else {
            Ok(())
        }
    }
}

fn word_to_value(word: &str, _span: Span) -> Result<MqlValue, MqlError> {
    if word.len() >= 2 && word.starts_with('/') && word.ends_with('/') {
        return Ok(MqlValue::Regex(word[1..word.len() - 1].to_string()));
    }
    Ok(MqlValue::Text(word.to_string()))
}
