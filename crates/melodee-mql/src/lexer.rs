//! MQL lexer. Produces span-tagged tokens for the parser.

use crate::ast::CompareOp;
use crate::errors::{MqlError, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    LParen,
    RParen,
    Colon,
    Range,
    Compare(CompareOp),
    Minus,
    /// Unquoted run; may be a field name, value, or boolean keyword.
    Word(String),
    /// Quoted string body (quotes stripped, escapes resolved).
    Quoted(String),
}

pub type SpannedToken = (Token, Span);

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')
}

fn is_ident_start(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenize `input`. Enforces the input-length and token-count bounds.
pub fn lex(
    input: &str,
    max_input_len: usize,
    max_tokens: usize,
) -> Result<Vec<SpannedToken>, MqlError> {
    if input.len() > max_input_len {
        return Err(MqlError::ExceedsBound {
            kind: "input length".to_string(),
            limit: max_input_len,
            span: None,
        });
    }
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut out = Vec::new();
    while cursor < chars.len() {
        let (start, c) = chars[cursor];
        let next_byte = |position: usize| {
            chars
                .get(position)
                .map_or(input.len(), |(byte_offset, _)| *byte_offset)
        };
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                cursor += 1;
            }
            '(' => {
                cursor += 1;
                out.push((Token::LParen, (start, next_byte(cursor))));
            }
            ')' => {
                cursor += 1;
                out.push((Token::RParen, (start, next_byte(cursor))));
            }
            ':' => {
                cursor += 1;
                out.push((Token::Colon, (start, next_byte(cursor))));
            }
            '"' | '\'' => {
                let quote = c;
                cursor += 1;
                let mut buf = String::new();
                let mut closed = false;
                while let Some((escape_start, ch)) = chars.get(cursor).copied() {
                    if ch == '\\' {
                        let Some((_, escaped)) = chars.get(cursor + 1).copied() else {
                            return Err(MqlError::Syntax {
                                message: "unterminated escape sequence".to_string(),
                                span: (escape_start, input.len()),
                            });
                        };
                        match escaped {
                            'n' => buf.push('\n'),
                            't' => buf.push('\t'),
                            'r' => buf.push('\r'),
                            '\\' => buf.push('\\'),
                            '"' => buf.push('"'),
                            '\'' => buf.push('\''),
                            other => {
                                return Err(MqlError::Syntax {
                                    message: format!("unknown escape sequence '\\{other}'"),
                                    span: (escape_start, next_byte(cursor + 2)),
                                });
                            }
                        }
                        cursor += 2;
                    } else if ch == quote {
                        closed = true;
                        cursor += 1;
                        break;
                    } else {
                        buf.push(ch);
                        cursor += 1;
                    }
                }
                if !closed {
                    return Err(MqlError::Syntax {
                        message: "unterminated quoted string".to_string(),
                        span: (start, input.len()),
                    });
                }
                out.push((Token::Quoted(buf), (start, next_byte(cursor))));
            }
            '-' => {
                // Leading hyphen becomes a negation prefix.
                cursor += 1;
                out.push((Token::Minus, (start, next_byte(cursor))));
            }
            '<' | '>' | '!' | '=' => {
                let op = if c == '=' {
                    CompareOp::Eq
                } else if c == '!' {
                    if matches!(chars.get(cursor + 1), Some((_, '='))) {
                        cursor += 1;
                        CompareOp::NotEq
                    } else {
                        return Err(MqlError::Syntax {
                            message: "expected '=' after '!'".to_string(),
                            span: (start, next_byte(cursor + 1)),
                        });
                    }
                } else if c == '<' {
                    if matches!(chars.get(cursor + 1), Some((_, '='))) {
                        cursor += 1;
                        CompareOp::Lte
                    } else {
                        CompareOp::Lt
                    }
                } else {
                    if matches!(chars.get(cursor + 1), Some((_, '='))) {
                        cursor += 1;
                        CompareOp::Gte
                    } else {
                        CompareOp::Gt
                    }
                };
                cursor += 1;
                out.push((Token::Compare(op), (start, next_byte(cursor))));
            }
            '.' => {
                // ".." is the inclusive-range operator.
                if matches!(chars.get(cursor + 1), Some((_, '.'))) {
                    cursor += 2;
                    out.push((Token::Range, (start, next_byte(cursor))));
                } else {
                    return Err(MqlError::Syntax {
                        message: "unexpected '.' (use '..' for ranges)".to_string(),
                        span: (start, next_byte(cursor + 1)),
                    });
                }
            }
            _ => {
                // A run beginning with '/' is a regex literal `/.../`; read until
                // the closing unescaped '/'.
                if c == '/' {
                    cursor += 1;
                    let mut escaped = false;
                    let mut closed = false;
                    while let Some((_, regex_char)) = chars.get(cursor).copied() {
                        if regex_char == '/' && !escaped {
                            cursor += 1;
                            closed = true;
                            break;
                        }
                        escaped = regex_char == '\\' && !escaped;
                        if regex_char != '\\' {
                            escaped = false;
                        }
                        cursor += 1;
                    }
                    if !closed {
                        return Err(MqlError::Syntax {
                            message: "unterminated regex literal".to_string(),
                            span: (start, input.len()),
                        });
                    }
                    let end = next_byte(cursor);
                    out.push((Token::Word(input[start..end].to_string()), (start, end)));
                } else {
                    // Read a word run; stop before ".." so ranges parse correctly.
                    while let Some((_, word_char)) = chars.get(cursor).copied() {
                        if word_char == '.' && matches!(chars.get(cursor + 1), Some((_, '.'))) {
                            break;
                        }
                        if !is_word_char(word_char) {
                            break;
                        }
                        cursor += 1;
                    }
                    let end = next_byte(cursor);
                    if end == start {
                        return Err(MqlError::Syntax {
                            message: format!("unexpected character '{c}'"),
                            span: (start, next_byte(cursor + 1)),
                        });
                    }
                    out.push((Token::Word(input[start..end].to_string()), (start, end)));
                }
            }
        }
        if out.len() > max_tokens {
            return Err(MqlError::ExceedsBound {
                kind: "token count".to_string(),
                limit: max_tokens,
                span: None,
            });
        }
    }
    Ok(out)
}

/// Whether a word token reads as a boolean keyword (case-insensitive).
pub fn is_bool_keyword(word: &str) -> Option<Keyword> {
    match word.to_ascii_uppercase().as_str() {
        "AND" => Some(Keyword::And),
        "OR" => Some(Keyword::Or),
        "NOT" => Some(Keyword::Not),
        _ => None,
    }
}

/// `AND`/`OR`/`NOT` keyword tokens, emitted by the parser from words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keyword {
    And,
    Or,
    Not,
}

pub fn ident_start(c: char) -> bool {
    is_ident_start(c)
}
