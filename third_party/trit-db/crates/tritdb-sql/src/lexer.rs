use crate::error::SqlError;

/// Reserved word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Create,
    Table,
    Alter,
    Add,
    If,
    Not,
    Exists,
    Drop,
    Insert,
    Into,
    Values,
    Select,
    From,
    Join,
    Asof,
    Inner,
    On,
    Using,
    Left,
    Right,
    Full,
    Cross,
    Natural,
    Outer,
    Where,
    Group,
    Having,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    As,
    And,
    Or,
    Is,
    In,
    Between,
    Like,
    Case,
    When,
    Then,
    Else,
    End,
    Null,
    True,
    False,
    Unknown,
    Delete,
    Update,
    Set,
    Explain,
    Analyze,
    Count,
    Sum,
    Avg,
    ProbSum,
    Min,
    Max,
    Int,
    Integer,
    Text,
    Bool,
    Boolean,
    With,
    Recursive,
    Union,
    All,
    Any,
}

impl Keyword {
    fn from_word(word: &str) -> Option<Self> {
        match word {
            "create" => Some(Keyword::Create),
            "table" => Some(Keyword::Table),
            "alter" => Some(Keyword::Alter),
            "add" => Some(Keyword::Add),
            "if" => Some(Keyword::If),
            "not" => Some(Keyword::Not),
            "exists" => Some(Keyword::Exists),
            "drop" => Some(Keyword::Drop),
            "insert" => Some(Keyword::Insert),
            "into" => Some(Keyword::Into),
            "values" => Some(Keyword::Values),
            "select" => Some(Keyword::Select),
            "from" => Some(Keyword::From),
            "join" => Some(Keyword::Join),
            "asof" => Some(Keyword::Asof),
            "inner" => Some(Keyword::Inner),
            "on" => Some(Keyword::On),
            "using" => Some(Keyword::Using),
            "left" => Some(Keyword::Left),
            "right" => Some(Keyword::Right),
            "full" => Some(Keyword::Full),
            "cross" => Some(Keyword::Cross),
            "natural" => Some(Keyword::Natural),
            "outer" => Some(Keyword::Outer),
            "where" => Some(Keyword::Where),
            "group" => Some(Keyword::Group),
            "having" => Some(Keyword::Having),
            "order" => Some(Keyword::Order),
            "by" => Some(Keyword::By),
            "asc" => Some(Keyword::Asc),
            "desc" => Some(Keyword::Desc),
            "limit" => Some(Keyword::Limit),
            "as" => Some(Keyword::As),
            "and" => Some(Keyword::And),
            "or" => Some(Keyword::Or),
            "is" => Some(Keyword::Is),
            "in" => Some(Keyword::In),
            "between" => Some(Keyword::Between),
            "like" => Some(Keyword::Like),
            "case" => Some(Keyword::Case),
            "when" => Some(Keyword::When),
            "then" => Some(Keyword::Then),
            "else" => Some(Keyword::Else),
            "end" => Some(Keyword::End),
            "null" => Some(Keyword::Null),
            "true" => Some(Keyword::True),
            "false" => Some(Keyword::False),
            "unknown" => Some(Keyword::Unknown),
            "delete" => Some(Keyword::Delete),
            "update" => Some(Keyword::Update),
            "set" => Some(Keyword::Set),
            "explain" => Some(Keyword::Explain),
            "analyze" => Some(Keyword::Analyze),
            "count" => Some(Keyword::Count),
            "sum" => Some(Keyword::Sum),
            "avg" => Some(Keyword::Avg),
            "prob_sum" => Some(Keyword::ProbSum),
            "min" => Some(Keyword::Min),
            "max" => Some(Keyword::Max),
            "int" => Some(Keyword::Int),
            "integer" => Some(Keyword::Integer),
            "text" => Some(Keyword::Text),
            "bool" => Some(Keyword::Bool),
            "boolean" => Some(Keyword::Boolean),
            "with" => Some(Keyword::With),
            "recursive" => Some(Keyword::Recursive),
            "union" => Some(Keyword::Union),
            "all" => Some(Keyword::All),
            "any" => Some(Keyword::Any),
            _ => None,
        }
    }
}

/// Punctuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// `,`.
    Comma,
    /// `.`.
    Dot,
    /// `;`.
    Semicolon,
    /// `*`.
    Star,
    /// `+`.
    Plus,
    /// `-`.
    Minus,
    /// `/`.
    Slash,
    /// `=`.
    Eq,
    /// `<>` or `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
}

/// Token payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Unquoted identifier, lowercased.
    Word(String),
    /// Quoted identifier, exact spelling.
    QuotedIdent(String),
    /// Integer literal.
    Number(i64),
    /// String literal.
    String(String),
    /// Reserved word.
    Keyword(Keyword),
    /// Punctuation.
    Symbol(Symbol),
}

/// Token with a character offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Token payload.
    pub kind: TokenKind,
    /// Character offset in the original input.
    pub offset: usize,
}

/// SQL lexer.
pub struct Lexer<'a> {
    chars: Vec<char>,
    pos: usize,
    _source: &'a str,
}

impl<'a> Lexer<'a> {
    /// Creates a lexer.
    pub fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            _source: source,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), SqlError> {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.pos += 1;
            }
            if self.peek() == Some('-') && self.peek_next() == Some('-') {
                self.pos += 2;
                while self.peek().is_some_and(|ch| ch != '\n') {
                    self.pos += 1;
                }
                continue;
            }
            if self.peek() == Some('/') && self.peek_next() == Some('*') {
                let start = self.pos;
                self.pos += 2;
                let mut closed = false;
                while let Some(ch) = self.bump() {
                    if ch == '*' && self.peek() == Some('/') {
                        self.pos += 1;
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(SqlError::parse("unterminated block comment", start));
                }
                continue;
            }
            return Ok(());
        }
    }

    fn read_ident(&mut self) -> String {
        let mut ident = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ident.push(ch);
                self.pos += 1;
            } else {
                break;
            }
        }
        ident.to_ascii_lowercase()
    }

    fn read_number(&mut self) -> Result<i64, SqlError> {
        let start = self.pos;
        let mut digits = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                digits.push(ch);
                self.pos += 1;
            } else {
                break;
            }
        }
        digits
            .parse::<i64>()
            .map_err(|_| SqlError::parse("integer literal out of range", start))
    }

    fn read_string(&mut self) -> Result<String, SqlError> {
        let start = self.pos;
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('\'') => {
                    if self.peek() == Some('\'') {
                        value.push('\'');
                        self.pos += 1;
                    } else {
                        return Ok(value);
                    }
                }
                Some(ch) => value.push(ch),
                None => return Err(SqlError::parse("unterminated string literal", start)),
            }
        }
    }

    fn read_quoted_ident(&mut self) -> Result<String, SqlError> {
        let start = self.pos;
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('"') => {
                    if self.peek() == Some('"') {
                        value.push('"');
                        self.pos += 1;
                    } else {
                        return Ok(value);
                    }
                }
                Some(ch) => value.push(ch),
                None => return Err(SqlError::parse("unterminated quoted identifier", start)),
            }
        }
    }

    /// Tokenizes the whole input.
    pub fn tokenize(mut self) -> Result<Vec<Token>, SqlError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments()?;
            let offset = self.pos;
            let Some(ch) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Symbol(Symbol::Semicolon),
                    offset,
                });
                break;
            };
            if ch.is_ascii_alphabetic() || ch == '_' {
                let word = self.read_ident();
                let kind =
                    Keyword::from_word(&word).map_or(TokenKind::Word(word), TokenKind::Keyword);
                tokens.push(Token { kind, offset });
                continue;
            }
            if ch.is_ascii_digit() {
                let number = self.read_number()?;
                tokens.push(Token {
                    kind: TokenKind::Number(number),
                    offset,
                });
                continue;
            }
            if ch == '\'' {
                let value = self.read_string()?;
                tokens.push(Token {
                    kind: TokenKind::String(value),
                    offset,
                });
                continue;
            }
            if ch == '"' {
                let value = self.read_quoted_ident()?;
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent(value),
                    offset,
                });
                continue;
            }

            self.pos += 1;
            let kind = match ch {
                '(' => TokenKind::Symbol(Symbol::LParen),
                ')' => TokenKind::Symbol(Symbol::RParen),
                ',' => TokenKind::Symbol(Symbol::Comma),
                '.' => TokenKind::Symbol(Symbol::Dot),
                ';' => TokenKind::Symbol(Symbol::Semicolon),
                '*' => TokenKind::Symbol(Symbol::Star),
                '+' => TokenKind::Symbol(Symbol::Plus),
                '-' => TokenKind::Symbol(Symbol::Minus),
                '/' => TokenKind::Symbol(Symbol::Slash),
                '=' => TokenKind::Symbol(Symbol::Eq),
                '<' => match self.peek() {
                    Some('=') => {
                        self.pos += 1;
                        TokenKind::Symbol(Symbol::Le)
                    }
                    Some('>') => {
                        self.pos += 1;
                        TokenKind::Symbol(Symbol::Ne)
                    }
                    _ => TokenKind::Symbol(Symbol::Lt),
                },
                '>' => match self.peek() {
                    Some('=') => {
                        self.pos += 1;
                        TokenKind::Symbol(Symbol::Ge)
                    }
                    _ => TokenKind::Symbol(Symbol::Gt),
                },
                '!' if self.peek() == Some('=') => {
                    self.pos += 1;
                    TokenKind::Symbol(Symbol::Ne)
                }
                other => {
                    return Err(SqlError::parse(
                        format!("unexpected character: {other}"),
                        offset,
                    ));
                }
            };
            tokens.push(Token { kind, offset });
        }
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_keywords_idents_and_literals() {
        let tokens = Lexer::new("SELECT a, \"B\" FROM t WHERE x <> 'it''s' AND n >= 3;")
            .tokenize()
            .expect("lex");
        assert_eq!(tokens[0].kind, TokenKind::Keyword(Keyword::Select));
        assert_eq!(tokens[1].kind, TokenKind::Word("a".to_string()));
        assert_eq!(tokens[2].kind, TokenKind::Symbol(Symbol::Comma));
        assert_eq!(tokens[3].kind, TokenKind::QuotedIdent("B".to_string()));
        assert!(tokens
            .iter()
            .any(|token| token.kind == TokenKind::String("it's".to_string())));
        assert!(tokens
            .iter()
            .any(|token| token.kind == TokenKind::Symbol(Symbol::Ne)));
    }

    #[test]
    fn skips_comments() {
        let tokens = Lexer::new("-- hi\nSELECT /* x */ 1;")
            .tokenize()
            .expect("lex");
        assert_eq!(tokens[0].kind, TokenKind::Keyword(Keyword::Select));
        assert_eq!(tokens[1].kind, TokenKind::Number(1));
    }
}
