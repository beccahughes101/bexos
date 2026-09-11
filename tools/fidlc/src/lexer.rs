#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Ident(String),
    IntLiteral(i64),
    StringLiteral(String),
    Symbol(char),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub pos: usize,
}

pub fn lex(source: &str) -> Result<Vec<Token>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let ch = bytes[cursor] as char;
        if ch.is_whitespace() {
            cursor += 1;
            continue;
        }
        if ch == '/' && bytes.get(cursor + 1) == Some(&b'/') {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if ch == '/' && bytes.get(cursor + 1) == Some(&b'*') {
            cursor += 2;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    cursor += 2;
                    break;
                }
                cursor += 1;
            }
            continue;
        }
        if is_ident_start(ch) {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_ident_continue(bytes[cursor] as char) {
                cursor += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident(source[start..cursor].to_string()),
                pos: start,
            });
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == '-'
                && bytes
                    .get(cursor + 1)
                    .is_some_and(|byte| byte.is_ascii_digit()))
        {
            let start = cursor;
            let negative = ch == '-';
            if negative {
                cursor += 1;
            }
            cursor += 1;
            if bytes.get(cursor - 1) == Some(&b'0')
                && matches!(bytes.get(cursor), Some(b'x' | b'X'))
            {
                cursor += 1;
            }
            while cursor < bytes.len() && bytes[cursor].is_ascii_hexdigit() {
                cursor += 1;
            }
            let literal = &source[start..cursor];
            let digits = if negative { &literal[1..] } else { literal };
            let mut value = if let Some(hex) = digits.strip_prefix("0x") {
                i64::from_str_radix(hex, 16)
            } else {
                digits.parse()
            }
            .map_err(|err| format!("invalid integer literal `{literal}` at byte {start}: {err}"))?;
            if negative {
                value = -value;
            }
            tokens.push(Token {
                kind: TokenKind::IntLiteral(value),
                pos: start,
            });
            continue;
        }
        if ch == '"' {
            let start = cursor;
            let literal = read_string(source, &mut cursor)?;
            tokens.push(Token {
                kind: TokenKind::StringLiteral(literal),
                pos: start,
            });
            continue;
        }

        tokens.push(Token {
            kind: TokenKind::Symbol(ch),
            pos: cursor,
        });
        cursor += 1;
    }

    Ok(tokens)
}

fn read_string(source: &str, cursor: &mut usize) -> Result<String, String> {
    let bytes = source.as_bytes();
    let start = *cursor;
    *cursor += 1;
    let mut literal = String::new();
    while *cursor < bytes.len() {
        let next = bytes[*cursor] as char;
        *cursor += 1;
        match next {
            '"' => return Ok(literal),
            '\\' => {
                if *cursor >= bytes.len() {
                    return Err(format!("unterminated escape in string at byte {start}"));
                }
                let escaped = bytes[*cursor] as char;
                *cursor += 1;
                literal.push(match escaped {
                    '"' => '"',
                    '\\' => '\\',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            other => literal.push(other),
        }
    }
    Err(format!("unterminated string literal at byte {start}"))
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}
