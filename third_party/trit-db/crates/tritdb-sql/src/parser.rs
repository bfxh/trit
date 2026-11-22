use tritdb_exec::{BinaryOp, DataType, TruthValue, Value};

use crate::ast::{
    AggregateKind, Assignment, ColumnDef, Cte, Expr, Join, JoinCondition, JoinKind, OrderBy,
    Quantifier, Query, Select, SelectItem, Statement, WindowFunction, With,
};
use crate::error::SqlError;
use crate::lexer::{Keyword, Lexer, Symbol, Token, TokenKind};

const MAX_EXPR_DEPTH: usize = 64;

/// Parses a semicolon-separated SQL script.
pub fn parse_script(sql: &str) -> Result<Vec<Statement>, SqlError> {
    let tokens = Lexer::new(sql).tokenize()?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        expr_depth: 0,
        source: sql.to_string(),
    };
    let mut statements = Vec::new();
    loop {
        while parser.eat_symbol(Symbol::Semicolon) {}
        if parser.at_end() {
            break;
        }
        statements.push(parser.parse_statement()?);
        if !parser.at_end() {
            parser.expect_symbol(Symbol::Semicolon)?;
        }
    }
    Ok(statements)
}

/// Parses a single expression, used for stored `CHECK` text.
pub fn parse_expression(sql: &str) -> Result<Expr, SqlError> {
    let mut tokens = Lexer::new(sql).tokenize()?;
    if matches!(
        tokens.last().map(|token| &token.kind),
        Some(TokenKind::Symbol(Symbol::Semicolon))
    ) {
        tokens.pop();
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        expr_depth: 0,
        source: sql.to_string(),
    };
    let expr = parser.parse_expr()?;
    if !parser.at_end() {
        return Err(parser.error("unexpected input after expression"));
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    expr_depth: usize,
    source: String,
}

impl Parser {
    fn with_expr_depth<F>(&mut self, parse: F) -> Result<Expr, SqlError>
    where
        F: FnOnce(&mut Self) -> Result<Expr, SqlError>,
    {
        if self.expr_depth >= MAX_EXPR_DEPTH {
            return Err(self.error("expression nesting too deep"));
        }
        self.expr_depth += 1;
        let result = parse(self);
        self.expr_depth -= 1;
        result
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    fn parse_check(&mut self) -> Result<String, SqlError> {
        self.expect_word("check")?;
        self.expect_symbol(Symbol::LParen)?;
        let start = self.offset();
        let _expr = self.parse_expr()?;
        let end = self.offset();
        self.expect_symbol(Symbol::RParen)?;
        Ok(self
            .source
            .get(start..end)
            .unwrap_or_default()
            .trim()
            .to_string())
    }

    fn peek_word(&self, word: &str) -> bool {
        matches!(
            self.peek().map(|token| &token.kind),
            Some(TokenKind::Word(name)) if name.eq_ignore_ascii_case(word)
        )
    }

    fn expect_word(&mut self, word: &str) -> Result<(), SqlError> {
        if self.eat_word(word) {
            Ok(())
        } else {
            Err(self.error(format!("expected {word}")))
        }
    }

    fn offset(&self) -> usize {
        self.peek().map_or(0, |token| token.offset)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned()?;
        self.pos += 1;
        Some(token)
    }

    fn error(&self, message: impl Into<String>) -> SqlError {
        SqlError::parse(message, self.offset())
    }

    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Keyword(keyword))
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self
            .peek()
            .is_some_and(|token| matches!(&token.kind, TokenKind::Word(value) if value == word))
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, keyword: Keyword) -> Result<(), SqlError> {
        if self.eat_keyword(keyword) {
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}", keyword)))
        }
    }

    fn eat_symbol(&mut self, symbol: Symbol) -> bool {
        if self
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Symbol(symbol))
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_symbol(&mut self, symbol: Symbol) -> Result<(), SqlError> {
        if self.eat_symbol(symbol) {
            Ok(())
        } else {
            Err(self.error(format!("expected {symbol:?}")))
        }
    }

    fn parse_ident(&mut self) -> Result<String, SqlError> {
        match self.bump().map(|token| token.kind) {
            Some(TokenKind::Word(name)) | Some(TokenKind::QuotedIdent(name)) => Ok(name),
            Some(_) => Err(self.error("expected identifier")),
            None => Err(self.error("expected identifier")),
        }
    }

    fn parse_type(&mut self) -> Result<DataType, SqlError> {
        let token = self.bump().ok_or_else(|| self.error("expected type"))?;
        let name = match token.kind {
            TokenKind::Keyword(Keyword::Int) | TokenKind::Keyword(Keyword::Integer) => {
                return Ok(DataType::Int);
            }
            TokenKind::Keyword(Keyword::Text) => return Ok(DataType::Text),
            TokenKind::Keyword(Keyword::Bool) | TokenKind::Keyword(Keyword::Boolean) => {
                return Ok(DataType::Bool);
            }
            TokenKind::Word(name) => name,
            _ => return Err(SqlError::parse("expected type name", token.offset)),
        };
        let mut data_type = DataType::from_name(&name)
            .ok_or_else(|| SqlError::parse(format!("unknown type: {name}"), token.offset))?;
        if data_type == DataType::Logic {
            if self.eat_word("belnap") {
                data_type = DataType::Belnap;
            } else if self.eat_word("lukasiewicz") {
                data_type = DataType::Lukasiewicz;
            } else if self.eat_word("nat") {
                data_type = DataType::Nat;
            } else {
                self.eat_word("k3");
            }
        }
        Ok(data_type)
    }

    fn parse_statement(&mut self) -> Result<Statement, SqlError> {
        match self.peek().map(|token| &token.kind) {
            Some(TokenKind::Keyword(Keyword::Create)) => self.parse_create(),
            Some(TokenKind::Keyword(Keyword::Alter)) => self.parse_alter(),
            Some(TokenKind::Keyword(Keyword::Drop)) => self.parse_drop(),
            Some(TokenKind::Keyword(Keyword::Insert)) => self.parse_insert(),
            Some(TokenKind::Keyword(Keyword::Select)) => self.parse_select(),
            Some(TokenKind::Keyword(Keyword::With)) => self.parse_with(),
            Some(TokenKind::Keyword(Keyword::Explain)) => self.parse_explain(),
            Some(TokenKind::Keyword(Keyword::Analyze)) => self.parse_analyze(),
            Some(TokenKind::Keyword(Keyword::Update)) => self.parse_update(),
            Some(TokenKind::Keyword(Keyword::Delete)) => self.parse_delete(),
            Some(_) => Err(self.error(
                "expected CREATE, ALTER, DROP, INSERT, SELECT, WITH, EXPLAIN, ANALYZE, UPDATE, or DELETE",
            )),
            None => Err(self.error("empty statement")),
        }
    }

    fn parse_with(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::With)?;
        let recursive = self.eat_keyword(Keyword::Recursive);
        let mut ctes = Vec::new();
        loop {
            let name = self.parse_ident()?;
            let mut columns = Vec::new();
            if self.eat_symbol(Symbol::LParen) {
                loop {
                    columns.push(self.parse_ident()?);
                    if self.eat_symbol(Symbol::Comma) {
                        continue;
                    }
                    break;
                }
                self.expect_symbol(Symbol::RParen)?;
            }
            self.expect_keyword(Keyword::As)?;
            self.expect_symbol(Symbol::LParen)?;
            let seed = self.parse_query()?;
            let (union_all, step) = if self.eat_keyword(Keyword::Union) {
                let union_all = self.eat_keyword(Keyword::All);
                (union_all, Some(Box::new(self.parse_query()?)))
            } else {
                (false, None)
            };
            self.expect_symbol(Symbol::RParen)?;
            ctes.push(Cte {
                name,
                columns,
                seed: Box::new(seed),
                step,
                union_all,
            });
            if self.eat_symbol(Symbol::Comma) {
                continue;
            }
            break;
        }
        let body = self.parse_statement()?;
        if !matches!(
            body,
            Statement::Select(_)
                | Statement::With(_)
                | Statement::Insert { .. }
                | Statement::Update { .. }
                | Statement::Delete { .. }
        ) {
            return Err(self.error("WITH must be followed by SELECT, INSERT, UPDATE, or DELETE"));
        }
        Ok(Statement::With(Box::new(With {
            recursive,
            ctes,
            body: Box::new(body),
        })))
    }

    fn parse_query(&mut self) -> Result<Query, SqlError> {
        match self.peek().map(|token| &token.kind) {
            Some(TokenKind::Keyword(Keyword::With)) => match self.parse_with()? {
                Statement::With(with) => Ok(Query::With(with)),
                _ => Err(self.error("expected WITH")),
            },
            Some(TokenKind::Keyword(Keyword::Select)) => {
                Ok(Query::Select(Box::new(self.parse_select_body()?)))
            }
            _ => Err(self.error("expected SELECT or WITH")),
        }
    }

    fn parse_select_body(&mut self) -> Result<Select, SqlError> {
        match self.parse_select()? {
            Statement::Select(select) => Ok(select),
            _ => Err(self.error("expected SELECT")),
        }
    }

    fn parse_create(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Create)?;
        self.expect_keyword(Keyword::Table)?;
        let mut if_not_exists = false;
        if self.eat_keyword(Keyword::If) {
            self.expect_keyword(Keyword::Not)?;
            self.expect_keyword(Keyword::Exists)?;
            if_not_exists = true;
        }
        let name = self.parse_ident()?;
        self.expect_symbol(Symbol::LParen)?;
        let mut columns = Vec::new();
        let mut unique_sets = Vec::new();
        let mut checks = Vec::new();
        loop {
            if self.peek_word("check") {
                checks.push(self.parse_check()?);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
            if self.peek_word("unique")
                && matches!(
                    self.peek_at(1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(Symbol::LParen))
                )
            {
                self.bump(); // unique
                self.expect_symbol(Symbol::LParen)?;
                let mut set = vec![self.parse_ident()?];
                while self.eat_symbol(Symbol::Comma) {
                    set.push(self.parse_ident()?);
                }
                self.expect_symbol(Symbol::RParen)?;
                unique_sets.push(set);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
            let column_name = self.parse_ident()?;
            let data_type = self.parse_type()?;
            let not_null = if self.eat_keyword(Keyword::Not) {
                self.expect_keyword(Keyword::Null)?;
                true
            } else {
                false
            };
            let unique = self.eat_word("unique");
            let check = if self.peek_word("check") {
                Some(self.parse_check()?)
            } else {
                None
            };
            columns.push(ColumnDef {
                name: column_name,
                data_type,
                not_null,
                unique,
                check,
            });
            if self.eat_symbol(Symbol::Comma) {
                continue;
            }
            break;
        }
        self.expect_symbol(Symbol::RParen)?;
        Ok(Statement::CreateTable {
            name,
            if_not_exists,
            columns,
            unique: unique_sets,
            checks,
        })
    }

    fn parse_drop(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Drop)?;
        self.expect_keyword(Keyword::Table)?;
        let mut if_exists = false;
        if self.eat_keyword(Keyword::If) {
            self.expect_keyword(Keyword::Exists)?;
            if_exists = true;
        }
        let name = self.parse_ident()?;
        Ok(Statement::DropTable { name, if_exists })
    }

    fn parse_alter(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Alter)?;
        self.expect_keyword(Keyword::Table)?;
        let name = self.parse_ident()?;
        if self.eat_keyword(Keyword::Drop) {
            self.eat_word("column");
            let if_exists = if self.eat_keyword(Keyword::If) {
                self.expect_keyword(Keyword::Exists)?;
                true
            } else {
                false
            };
            let column = self.parse_ident()?;
            return Ok(Statement::AlterTableDropColumn {
                name,
                if_exists,
                column,
            });
        }
        if self.eat_word("rename") {
            if self.eat_word("to") {
                let to = self.parse_ident()?;
                return Ok(Statement::RenameTable { from: name, to });
            }
            self.eat_word("column");
            let from = self.parse_ident()?;
            self.expect_word("to")?;
            let to = self.parse_ident()?;
            return Ok(Statement::AlterTableRenameColumn { name, from, to });
        }
        if self.eat_keyword(Keyword::Alter) {
            self.eat_word("column");
            let column = self.parse_ident()?;
            self.eat_word("type");
            let data_type = self.parse_type()?;
            return Ok(Statement::AlterColumnType {
                table: name,
                column,
                data_type,
            });
        }
        self.expect_keyword(Keyword::Add)?;
        self.eat_word("column");
        let mut if_not_exists = false;
        if self.eat_keyword(Keyword::If) {
            self.expect_keyword(Keyword::Not)?;
            self.expect_keyword(Keyword::Exists)?;
            if_not_exists = true;
        }
        let column_name = self.parse_ident()?;
        let data_type = self.parse_type()?;
        let not_null = if self.eat_keyword(Keyword::Not) {
            self.expect_keyword(Keyword::Null)?;
            true
        } else {
            false
        };
        Ok(Statement::AlterTableAddColumn {
            name,
            if_not_exists,
            column: ColumnDef {
                name: column_name,
                data_type,
                not_null,
                unique: self.eat_word("unique"),
                check: if self.peek_word("check") {
                    Some(self.parse_check()?)
                } else {
                    None
                },
            },
        })
    }

    fn parse_insert(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Insert)?;
        self.expect_keyword(Keyword::Into)?;
        let table = self.parse_ident()?;
        let mut columns = None;
        if self.eat_symbol(Symbol::LParen) {
            let mut names = Vec::new();
            loop {
                names.push(self.parse_ident()?);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
            self.expect_symbol(Symbol::RParen)?;
            columns = Some(names);
        }
        let select = if matches!(
            self.peek().map(|token| &token.kind),
            Some(TokenKind::Keyword(Keyword::Select)) | Some(TokenKind::Keyword(Keyword::With))
        ) {
            Some(Box::new(self.parse_query()?))
        } else {
            None
        };
        let mut rows = Vec::new();
        if select.is_none() {
            self.expect_keyword(Keyword::Values)?;
            loop {
                self.expect_symbol(Symbol::LParen)?;
                let mut values = Vec::new();
                loop {
                    values.push(self.parse_expr()?);
                    if self.eat_symbol(Symbol::Comma) {
                        continue;
                    }
                    break;
                }
                self.expect_symbol(Symbol::RParen)?;
                rows.push(values);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }
        Ok(Statement::Insert {
            table,
            columns,
            rows,
            select,
        })
    }

    fn parse_explain(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Explain)?;
        let analyze = self.eat_keyword(Keyword::Analyze);
        let sexpr = self.eat_word("sexpr");
        let select = match self.parse_select()? {
            Statement::Select(select) => select,
            _ => unreachable!("parse_select always returns SELECT"),
        };
        Ok(if analyze && sexpr {
            Statement::ExplainAnalyzeSexpr(select)
        } else if analyze {
            Statement::ExplainAnalyze(select)
        } else if sexpr {
            Statement::ExplainSexpr(select)
        } else {
            Statement::Explain(select)
        })
    }

    fn parse_analyze(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Analyze)?;
        let table = match self.peek().map(|token| &token.kind) {
            Some(TokenKind::Word(_)) | Some(TokenKind::QuotedIdent(_)) => Some(self.parse_ident()?),
            _ => None,
        };
        Ok(Statement::Analyze { table })
    }

    fn parse_select(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Select)?;

        // `SELECT count_models('<expr>'[, vars])` is a rule-level statement
        // without a `FROM` clause. Check the second token so a column named
        // `count_models` still parses as an ordinary select item.
        if let Some(Token {
            kind: TokenKind::Word(name),
            ..
        }) = self.peek()
        {
            let is_call = name.eq_ignore_ascii_case("count_models")
                && matches!(
                    self.peek_at(1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(Symbol::LParen))
                );
            if is_call {
                return self.parse_count_models();
            }
        }

        let mut items = Vec::new();
        if self.eat_symbol(Symbol::Star) {
            items.push(SelectItem::Wildcard);
        } else {
            loop {
                let expr = self.parse_expr()?;
                let alias = if self.eat_keyword(Keyword::As) {
                    Some(self.parse_ident()?)
                } else {
                    match self.peek().map(|token| &token.kind) {
                        Some(TokenKind::Word(_)) | Some(TokenKind::QuotedIdent(_)) => {
                            Some(self.parse_ident()?)
                        }
                        _ => None,
                    }
                };
                items.push(SelectItem::Expr { expr, alias });
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }

        let (table, table_alias) = if self.eat_keyword(Keyword::From) {
            self.parse_table_ref()?
        } else {
            (String::new(), None)
        };

        let mut joins = Vec::new();
        loop {
            if matches!(
                self.peek().map(|token| &token.kind),
                Some(TokenKind::Keyword(Keyword::Outer))
            ) {
                return Err(SqlError::Unsupported(
                    "OUTER must follow LEFT, RIGHT, or FULL".to_string(),
                ));
            }

            if self.eat_symbol(Symbol::Comma) {
                let (join_table, alias) = self.parse_table_ref()?;
                joins.push(Join {
                    kind: JoinKind::Cross,
                    table: join_table,
                    alias,
                    condition: JoinCondition::None,
                });
                continue;
            }

            let natural = self.eat_keyword(Keyword::Natural);
            let kind = if self.eat_keyword(Keyword::Inner) {
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Inner
            } else if self.eat_keyword(Keyword::Cross) {
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Cross
            } else if self.eat_keyword(Keyword::Left) {
                self.eat_keyword(Keyword::Outer);
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Left
            } else if self.eat_keyword(Keyword::Right) {
                self.eat_keyword(Keyword::Outer);
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Right
            } else if self.eat_keyword(Keyword::Full) {
                self.eat_keyword(Keyword::Outer);
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Full
            } else if self.eat_keyword(Keyword::Asof) {
                self.expect_keyword(Keyword::Join)?;
                JoinKind::Asof
            } else if self.eat_keyword(Keyword::Join) {
                JoinKind::Inner
            } else if natural {
                return Err(self.error("expected JOIN after NATURAL"));
            } else {
                break;
            };

            if natural && kind == JoinKind::Cross {
                return Err(SqlError::Unsupported(
                    "NATURAL CROSS JOIN is not supported".to_string(),
                ));
            }
            if natural && kind == JoinKind::Asof {
                return Err(SqlError::Unsupported(
                    "NATURAL ASOF JOIN is not supported".to_string(),
                ));
            }

            let (join_table, alias) = self.parse_table_ref()?;
            let condition = if natural {
                if self.eat_keyword(Keyword::On) || self.eat_keyword(Keyword::Using) {
                    return Err(self.error("NATURAL JOIN cannot take ON or USING"));
                }
                JoinCondition::Natural
            } else if kind == JoinKind::Cross {
                if self.eat_keyword(Keyword::On) || self.eat_keyword(Keyword::Using) {
                    return Err(SqlError::Unsupported(
                        "CROSS JOIN does not take ON or USING".to_string(),
                    ));
                }
                JoinCondition::None
            } else if kind == JoinKind::Asof {
                self.expect_keyword(Keyword::On)?;
                JoinCondition::On(self.parse_expr()?)
            } else if self.eat_keyword(Keyword::Using) {
                self.expect_symbol(Symbol::LParen)?;
                let mut columns = Vec::new();
                loop {
                    columns.push(self.parse_ident()?);
                    if self.eat_symbol(Symbol::Comma) {
                        continue;
                    }
                    break;
                }
                self.expect_symbol(Symbol::RParen)?;
                if self.eat_keyword(Keyword::On) {
                    return Err(self.error("JOIN USING cannot take ON"));
                }
                JoinCondition::Using(columns)
            } else {
                self.expect_keyword(Keyword::On)?;
                JoinCondition::On(self.parse_expr()?)
            };
            joins.push(Join {
                kind,
                table: join_table,
                alias,
                condition,
            });
        }

        let under_repair = if self.eat_word("under") {
            self.expect_word("repair")?;
            self.expect_keyword(Keyword::On)?;
            self.expect_symbol(Symbol::LParen)?;
            let mut keys = vec![self.parse_ident()?];
            while self.eat_symbol(Symbol::Comma) {
                keys.push(self.parse_ident()?);
            }
            self.expect_symbol(Symbol::RParen)?;
            Some(keys)
        } else {
            None
        };

        let filter = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let mut group_by = Vec::new();
        if self.eat_keyword(Keyword::Group) {
            self.expect_keyword(Keyword::By)?;
            loop {
                group_by.push(self.parse_expr()?);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }

        let having = if self.eat_keyword(Keyword::Having) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let mut order_by = Vec::new();
        if self.eat_keyword(Keyword::Order) {
            self.expect_keyword(Keyword::By)?;
            loop {
                let expr = self.parse_expr()?;
                let ascending = if self.eat_keyword(Keyword::Desc) {
                    false
                } else {
                    self.eat_keyword(Keyword::Asc);
                    true
                };
                order_by.push(OrderBy { expr, ascending });
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }

        let limit = if self.eat_keyword(Keyword::Limit) {
            match self.bump().map(|token| token.kind) {
                Some(TokenKind::Number(value)) if value >= 0 => Some(value as usize),
                Some(TokenKind::Number(_)) => return Err(self.error("LIMIT must be non-negative")),
                _ => return Err(self.error("LIMIT requires an integer")),
            }
        } else {
            None
        };

        Ok(Statement::Select(Select {
            items,
            table,
            table_alias,
            joins,
            filter,
            group_by,
            having,
            order_by,
            limit,
            under_repair,
        }))
    }

    fn parse_count_models(&mut self) -> Result<Statement, SqlError> {
        self.bump(); // count_models
        self.expect_symbol(Symbol::LParen)?;
        let expr = match self.bump() {
            Some(Token {
                kind: TokenKind::String(value),
                ..
            }) => value,
            Some(token) => {
                return Err(SqlError::parse(
                    "count_models expects a string literal",
                    token.offset,
                ))
            }
            None => return Err(self.error("count_models expects a string literal")),
        };
        let vars = if self.eat_symbol(Symbol::Comma) {
            match self.bump() {
                Some(Token {
                    kind: TokenKind::Number(value),
                    ..
                }) if value >= 0 => value as usize,
                Some(token) => {
                    return Err(SqlError::parse(
                        "count_models variable count must be a non-negative integer",
                        token.offset,
                    ))
                }
                None => return Err(self.error("count_models expects a variable count")),
            }
        } else {
            0
        };
        self.expect_symbol(Symbol::RParen)?;
        Ok(Statement::CountModels { expr, vars })
    }

    fn parse_table_ref(&mut self) -> Result<(String, Option<String>), SqlError> {
        let table = self.parse_ident()?;
        let alias = if self.eat_keyword(Keyword::As) {
            Some(self.parse_ident()?)
        } else {
            match self.peek().map(|token| &token.kind) {
                // `UNDER REPAIR` starts right after the table (and aliases).
                Some(TokenKind::Word(word)) if !word.eq_ignore_ascii_case("under") => {
                    Some(self.parse_ident()?)
                }
                Some(TokenKind::QuotedIdent(_)) => Some(self.parse_ident()?),
                _ => None,
            }
        };
        Ok((table, alias))
    }

    fn parse_update(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Update)?;
        let table = self.parse_ident()?;
        self.expect_keyword(Keyword::Set)?;
        let mut assignments = Vec::new();
        loop {
            let column = self.parse_ident()?;
            self.expect_symbol(Symbol::Eq)?;
            let value = self.parse_expr()?;
            assignments.push(Assignment { column, value });
            if self.eat_symbol(Symbol::Comma) {
                continue;
            }
            break;
        }
        let filter = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Statement::Update {
            table,
            assignments,
            filter,
        })
    }

    fn parse_delete(&mut self) -> Result<Statement, SqlError> {
        self.expect_keyword(Keyword::Delete)?;
        self.expect_keyword(Keyword::From)?;
        let table = self.parse_ident()?;
        let filter = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Statement::Delete { table, filter })
    }

    fn parse_expr(&mut self) -> Result<Expr, SqlError> {
        self.with_expr_depth(|parser| parser.parse_or())
    }

    fn parse_or(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.parse_and()?;
        while self.eat_keyword(Keyword::Or) {
            let right = self.parse_and()?;
            left = binary(BinaryOp::Or, left, right);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.parse_not()?;
        while self.eat_keyword(Keyword::And) {
            let right = self.parse_not()?;
            left = binary(BinaryOp::And, left, right);
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, SqlError> {
        if self.eat_keyword(Keyword::Not) {
            self.with_expr_depth(|parser| Ok(Expr::Not(Box::new(parser.parse_not()?))))
        } else {
            self.parse_comparison()
        }
    }

    fn parse_comparison(&mut self) -> Result<Expr, SqlError> {
        let left = self.parse_additive()?;

        if self.eat_keyword(Keyword::Is) {
            let negated = self.eat_keyword(Keyword::Not);
            let expected = if self.eat_keyword(Keyword::Null) {
                return Ok(Expr::IsNull {
                    expr: Box::new(left),
                    negated,
                });
            } else if self.eat_keyword(Keyword::True) {
                TruthValue::True
            } else if self.eat_keyword(Keyword::False) {
                TruthValue::False
            } else if self.eat_keyword(Keyword::Unknown) {
                TruthValue::Unknown
            } else {
                return Err(self.error("expected NULL, TRUE, FALSE, or UNKNOWN after IS"));
            };
            return Ok(Expr::IsTruth {
                expr: Box::new(left),
                expected,
                negated,
            });
        }

        let negated = if self.eat_keyword(Keyword::Not) {
            if matches!(
                self.peek().map(|token| &token.kind),
                Some(TokenKind::Keyword(
                    Keyword::In | Keyword::Between | Keyword::Like
                ))
            ) {
                true
            } else {
                return Err(self.error("expected IN, BETWEEN, or LIKE after NOT"));
            }
        } else {
            false
        };

        if self.eat_keyword(Keyword::In) {
            self.expect_symbol(Symbol::LParen)?;
            if matches!(
                self.peek().map(|token| &token.kind),
                Some(TokenKind::Keyword(Keyword::Select)) | Some(TokenKind::Keyword(Keyword::With))
            ) {
                let query = self.parse_query()?;
                self.expect_symbol(Symbol::RParen)?;
                return Ok(match left {
                    Expr::Tuple(exprs) => Expr::InSubqueryTuple {
                        exprs,
                        query: Box::new(query),
                        negated,
                    },
                    other => Expr::InSubquery {
                        expr: Box::new(other),
                        query: Box::new(query),
                        negated,
                    },
                });
            }
            let mut values = Vec::new();
            loop {
                values.push(self.parse_expr()?);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
            self.expect_symbol(Symbol::RParen)?;
            return Ok(Expr::InList {
                expr: Box::new(left),
                values,
                negated,
            });
        }

        if self.eat_keyword(Keyword::Between) {
            let low = self.parse_additive()?;
            self.expect_keyword(Keyword::And)?;
            let high = self.parse_additive()?;
            return Ok(Expr::Between {
                expr: Box::new(left),
                low: Box::new(low),
                high: Box::new(high),
                negated,
            });
        }

        if self.eat_keyword(Keyword::Like) {
            let pattern = self.parse_additive()?;
            return Ok(Expr::Like {
                expr: Box::new(left),
                pattern: Box::new(pattern),
                negated,
            });
        }

        let op = match self.peek().map(|token| &token.kind) {
            Some(TokenKind::Symbol(Symbol::Eq)) => Some(BinaryOp::Eq),
            Some(TokenKind::Symbol(Symbol::Ne)) => Some(BinaryOp::Ne),
            Some(TokenKind::Symbol(Symbol::Lt)) => Some(BinaryOp::Lt),
            Some(TokenKind::Symbol(Symbol::Le)) => Some(BinaryOp::Le),
            Some(TokenKind::Symbol(Symbol::Gt)) => Some(BinaryOp::Gt),
            Some(TokenKind::Symbol(Symbol::Ge)) => Some(BinaryOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let kind = if self.eat_keyword(Keyword::Any) {
                Some(Quantifier::Any)
            } else if self.eat_keyword(Keyword::All) {
                Some(Quantifier::All)
            } else {
                None
            };
            if let Some(kind) = kind {
                self.expect_symbol(Symbol::LParen)?;
                let query = self.parse_query()?;
                self.expect_symbol(Symbol::RParen)?;
                return Ok(Expr::Quantified {
                    expr: Box::new(left),
                    op,
                    kind,
                    query: Box::new(query),
                });
            }
            let right = self.parse_additive()?;
            return Ok(binary(op, left, right));
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek().map(|token| &token.kind) {
                Some(TokenKind::Symbol(Symbol::Plus)) => BinaryOp::Add,
                Some(TokenKind::Symbol(Symbol::Minus)) => BinaryOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_multiplicative()?;
            left = binary(op, left, right);
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().map(|token| &token.kind) {
                Some(TokenKind::Symbol(Symbol::Star)) => BinaryOp::Mul,
                Some(TokenKind::Symbol(Symbol::Slash)) => BinaryOp::Div,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            left = binary(op, left, right);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, SqlError> {
        if self.eat_symbol(Symbol::Minus) {
            if let Some(Token {
                kind: TokenKind::Number(value),
                ..
            }) = self.peek().cloned()
            {
                self.pos += 1;
                return Ok(Expr::Literal(Value::Int(-value)));
            }
            return self.with_expr_depth(|parser| {
                Ok(binary(
                    BinaryOp::Sub,
                    Expr::Literal(Value::Int(0)),
                    parser.parse_unary()?,
                ))
            });
        }
        self.eat_symbol(Symbol::Plus);
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, SqlError> {
        let token = self
            .bump()
            .ok_or_else(|| self.error("expected expression"))?;
        match token.kind {
            TokenKind::Number(value) => Ok(Expr::Literal(Value::Int(value))),
            TokenKind::String(value) => Ok(Expr::Literal(Value::Text(value))),
            TokenKind::Word(name) | TokenKind::QuotedIdent(name) => {
                if name.eq_ignore_ascii_case("both") {
                    return Ok(Expr::Literal(Value::Logic(tritdb_exec::ann::B4::Both)));
                }
                if name.eq_ignore_ascii_case("neither") {
                    return Ok(Expr::Literal(Value::Logic(tritdb_exec::ann::B4::Neither)));
                }
                // `truth(x [, threshold])` projects an annotation to
                // TRUE / FALSE / UNKNOWN. K3/Belnap use the truth tables;
                // Łukasiewicz and ℕ compare against a threshold.
                if name.eq_ignore_ascii_case("truth")
                    && self
                        .peek()
                        .is_some_and(|token| token.kind == TokenKind::Symbol(Symbol::LParen))
                {
                    self.bump(); // (
                    let inner = self.parse_expr()?;
                    let threshold = if self.eat_symbol(Symbol::Comma) {
                        Some(Box::new(self.parse_expr()?))
                    } else {
                        None
                    };
                    self.expect_symbol(Symbol::RParen)?;
                    return Ok(Expr::Truth {
                        expr: Box::new(inner),
                        threshold,
                    });
                }
                // `certainty(x [, threshold])` reports whether the
                // annotation forces the statement: CERTAIN / CONTINGENT /
                // IMPOSSIBLE, matching `UNDER REPAIR`'s answer vocabulary.
                if name.eq_ignore_ascii_case("certainty")
                    && self
                        .peek()
                        .is_some_and(|token| token.kind == TokenKind::Symbol(Symbol::LParen))
                {
                    self.bump(); // (
                    let inner = self.parse_expr()?;
                    let threshold = if self.eat_symbol(Symbol::Comma) {
                        Some(Box::new(self.parse_expr()?))
                    } else {
                        None
                    };
                    self.expect_symbol(Symbol::RParen)?;
                    return Ok(Expr::Certainty {
                        expr: Box::new(inner),
                        threshold,
                    });
                }
                if let Some(function) = window_function(&name) {
                    if self
                        .peek()
                        .is_some_and(|token| token.kind == TokenKind::Symbol(Symbol::LParen))
                    {
                        self.expect_symbol(Symbol::LParen)?;
                        let input =
                            if matches!(function, WindowFunction::Lag | WindowFunction::Lead) {
                                Some(Box::new(self.parse_expr()?))
                            } else {
                                None
                            };
                        self.expect_symbol(Symbol::RParen)?;
                        return self.parse_window(function, input);
                    }
                }
                if self.eat_symbol(Symbol::Dot) {
                    let column = self.parse_ident()?;
                    Ok(Expr::Column {
                        table: Some(name),
                        name: column,
                    })
                } else {
                    Ok(Expr::Column { table: None, name })
                }
            }
            TokenKind::Keyword(Keyword::True) => Ok(Expr::Literal(Value::Bool(true))),
            TokenKind::Keyword(Keyword::False) => Ok(Expr::Literal(Value::Bool(false))),
            TokenKind::Keyword(Keyword::Null) | TokenKind::Keyword(Keyword::Unknown) => {
                Ok(Expr::Literal(Value::Null))
            }
            TokenKind::Keyword(Keyword::Count) => self.parse_aggregate(AggregateKind::Count),
            TokenKind::Keyword(Keyword::Sum) => self.parse_aggregate(AggregateKind::Sum),
            TokenKind::Keyword(Keyword::Avg) => self.parse_aggregate(AggregateKind::Avg),
            TokenKind::Keyword(Keyword::ProbSum) => self.parse_aggregate(AggregateKind::ProbSum),
            TokenKind::Keyword(Keyword::Min) => self.parse_aggregate(AggregateKind::Min),
            TokenKind::Keyword(Keyword::Max) => self.parse_aggregate(AggregateKind::Max),
            TokenKind::Keyword(Keyword::Case) => self.parse_case(),
            TokenKind::Keyword(Keyword::Exists) => self.parse_exists(),
            TokenKind::Symbol(Symbol::LParen) => {
                if matches!(
                    self.peek().map(|token| &token.kind),
                    Some(TokenKind::Keyword(Keyword::Select))
                        | Some(TokenKind::Keyword(Keyword::With))
                ) {
                    let query = self.parse_query()?;
                    self.expect_symbol(Symbol::RParen)?;
                    return Ok(Expr::ScalarSubquery {
                        query: Box::new(query),
                    });
                }
                let first = self.parse_expr()?;
                if self.eat_symbol(Symbol::Comma) {
                    let mut exprs = vec![first];
                    loop {
                        exprs.push(self.parse_expr()?);
                        if self.eat_symbol(Symbol::Comma) {
                            continue;
                        }
                        break;
                    }
                    self.expect_symbol(Symbol::RParen)?;
                    return Ok(Expr::Tuple(exprs));
                }
                self.expect_symbol(Symbol::RParen)?;
                Ok(first)
            }
            _ => Err(SqlError::parse("expected expression", token.offset)),
        }
    }

    fn parse_exists(&mut self) -> Result<Expr, SqlError> {
        self.expect_symbol(Symbol::LParen)?;
        let query = self.parse_query()?;
        self.expect_symbol(Symbol::RParen)?;
        Ok(Expr::Exists {
            query: Box::new(query),
        })
    }

    fn parse_case(&mut self) -> Result<Expr, SqlError> {
        let operand = if self
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Keyword(Keyword::When))
        {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        self.expect_keyword(Keyword::When)?;
        let mut branches = Vec::new();
        loop {
            let when = self.parse_expr()?;
            self.expect_keyword(Keyword::Then)?;
            let then = self.parse_expr()?;
            branches.push((when, then));
            if !self.eat_keyword(Keyword::When) {
                break;
            }
        }
        let else_expr = if self.eat_keyword(Keyword::Else) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword(Keyword::End)?;
        Ok(Expr::Case {
            operand,
            branches,
            else_expr,
        })
    }

    fn parse_window(
        &mut self,
        function: WindowFunction,
        input: Option<Box<Expr>>,
    ) -> Result<Expr, SqlError> {
        self.expect_word("over")?;
        self.expect_symbol(Symbol::LParen)?;
        let mut partition_by = Vec::new();
        if self.eat_word("partition") {
            self.expect_keyword(Keyword::By)?;
            loop {
                partition_by.push(self.parse_expr()?);
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }
        let mut order_by = Vec::new();
        if self.eat_keyword(Keyword::Order) {
            self.expect_keyword(Keyword::By)?;
            loop {
                let expr = self.parse_expr()?;
                let ascending = if self.eat_keyword(Keyword::Desc) {
                    false
                } else {
                    self.eat_keyword(Keyword::Asc);
                    true
                };
                order_by.push(OrderBy { expr, ascending });
                if self.eat_symbol(Symbol::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect_symbol(Symbol::RParen)?;
        Ok(Expr::Window {
            function,
            input,
            partition_by,
            order_by,
        })
    }

    fn parse_aggregate(&mut self, kind: AggregateKind) -> Result<Expr, SqlError> {
        self.expect_symbol(Symbol::LParen)?;
        let input = if kind == AggregateKind::Count && self.eat_symbol(Symbol::Star) {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        self.expect_symbol(Symbol::RParen)?;
        if self.peek_word("over") {
            return self.parse_window(WindowFunction::Aggregate(kind), input);
        }
        Ok(Expr::Aggregate { kind, input })
    }
}

fn window_function(name: &str) -> Option<WindowFunction> {
    if name.eq_ignore_ascii_case("row_number") {
        Some(WindowFunction::RowNumber)
    } else if name.eq_ignore_ascii_case("rank") {
        Some(WindowFunction::Rank)
    } else if name.eq_ignore_ascii_case("dense_rank") {
        Some(WindowFunction::DenseRank)
    } else if name.eq_ignore_ascii_case("lag") {
        Some(WindowFunction::Lag)
    } else if name.eq_ignore_ascii_case("lead") {
        Some(WindowFunction::Lead)
    } else {
        None
    }
}

fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_create_and_insert() {
        let statements = parse_script(
            "CREATE TABLE t (id INT NOT NULL, name TEXT, ok BOOL); \
             INSERT INTO t (id, name, ok) VALUES (1, 'a', TRUE), (2, NULL, UNKNOWN);",
        )
        .expect("parse");
        assert_eq!(statements.len(), 2);
        match &statements[0] {
            Statement::CreateTable { name, columns, .. } => {
                assert_eq!(name, "t");
                assert_eq!(columns.len(), 3);
                assert!(columns[0].not_null);
            }
            other => panic!("unexpected {other:?}"),
        }
        match &statements[1] {
            Statement::Insert { table, rows, .. } => {
                assert_eq!(table, "t");
                assert_eq!(rows.len(), 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_alter_table_add_column() {
        let statements = parse_script(
            "ALTER TABLE t ADD COLUMN score INT; \
             ALTER TABLE t ADD IF NOT EXISTS flag BOOL NOT NULL;",
        )
        .expect("parse alter");
        match &statements[0] {
            Statement::AlterTableAddColumn {
                name,
                if_not_exists,
                column,
            } => {
                assert_eq!(name, "t");
                assert!(!if_not_exists);
                assert_eq!(column.name, "score");
                assert_eq!(column.data_type, DataType::Int);
                assert!(!column.not_null);
            }
            other => panic!("unexpected {other:?}"),
        }
        match &statements[1] {
            Statement::AlterTableAddColumn {
                name,
                if_not_exists,
                column,
            } => {
                assert_eq!(name, "t");
                assert!(if_not_exists);
                assert_eq!(column.name, "flag");
                assert_eq!(column.data_type, DataType::Bool);
                assert!(column.not_null);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_update_assignments_and_where() {
        let statements = parse_script("UPDATE t SET score = score + 1, name = NULL WHERE id >= 2;")
            .expect("parse");
        match &statements[0] {
            Statement::Update {
                table,
                assignments,
                filter,
            } => {
                assert_eq!(table, "t");
                assert_eq!(assignments.len(), 2);
                assert_eq!(assignments[0].column, "score");
                assert_eq!(assignments[1].column, "name");
                assert!(filter.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_analyze_table_or_all() {
        let statements = parse_script("ANALYZE; ANALYZE t;").expect("parse analyze");
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0], Statement::Analyze { table: None });
        assert_eq!(
            statements[1],
            Statement::Analyze {
                table: Some("t".to_string())
            }
        );
    }

    #[test]
    fn parses_explain_select() {
        for (sql, analyze) in [
            ("EXPLAIN SELECT a.id FROM a JOIN b ON a.id = b.id;", false),
            (
                "EXPLAIN ANALYZE SELECT a.id FROM a JOIN b ON a.id = b.id;",
                true,
            ),
        ] {
            let statements = parse_script(sql).expect("parse explain");
            assert_eq!(statements.len(), 1);
            match &statements[0] {
                Statement::Explain(select) => {
                    assert!(!analyze);
                    assert_eq!(select.table, "a");
                    assert_eq!(select.joins.len(), 1);
                    assert_eq!(select.joins[0].table, "b");
                }
                Statement::ExplainAnalyze(select) => {
                    assert!(analyze);
                    assert_eq!(select.table, "a");
                    assert_eq!(select.joins.len(), 1);
                    assert_eq!(select.joins[0].table, "b");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn parses_explain_sexpr_select() {
        let statements = parse_script("EXPLAIN SEXPR SELECT a.id FROM a JOIN b ON a.id = b.id;")
            .expect("parse explain sexpr");
        assert_eq!(statements.len(), 1);
        match &statements[0] {
            Statement::ExplainSexpr(select) => {
                assert_eq!(select.table, "a");
                assert_eq!(select.joins.len(), 1);
                assert_eq!(select.joins[0].table, "b");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_explain_analyze_sexpr_select() {
        let statements =
            parse_script("EXPLAIN ANALYZE SEXPR SELECT a.id FROM a JOIN b ON a.id = b.id;")
                .expect("parse explain analyze sexpr");
        assert_eq!(statements.len(), 1);
        match &statements[0] {
            Statement::ExplainAnalyzeSexpr(select) => {
                assert_eq!(select.table, "a");
                assert_eq!(select.joins.len(), 1);
                assert_eq!(select.joins[0].table, "b");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_in_between_and_like() {
        for (sql, expected) in [
            (
                "SELECT id FROM t WHERE id IN (1, 2);",
                Predicate::InList(false),
            ),
            (
                "SELECT id FROM t WHERE id NOT IN (1, 2);",
                Predicate::InList(true),
            ),
            (
                "SELECT id FROM t WHERE score BETWEEN 10 AND 20;",
                Predicate::Between(false),
            ),
            (
                "SELECT id FROM t WHERE score NOT BETWEEN 10 AND 20;",
                Predicate::Between(true),
            ),
            (
                "SELECT id FROM t WHERE name LIKE 'a%';",
                Predicate::Like(false),
            ),
            (
                "SELECT id FROM t WHERE name NOT LIKE '_';",
                Predicate::Like(true),
            ),
        ] {
            let statements = parse_script(sql).expect("parse predicate");
            let Statement::Select(select) = &statements[0] else {
                panic!("expected select");
            };
            let actual = match select.filter.as_ref().expect("filter") {
                Expr::InList { negated, .. } => Predicate::InList(*negated),
                Expr::Between { negated, .. } => Predicate::Between(*negated),
                Expr::Like { negated, .. } => Predicate::Like(*negated),
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(actual, expected, "{sql}");
        }
        assert!(parse_script("SELECT id FROM t WHERE id NOT 1;").is_err());
        assert!(parse_script("SELECT id FROM t WHERE id IN ();").is_err());
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Predicate {
        InList(bool),
        Between(bool),
        Like(bool),
    }

    #[test]
    fn parses_select_with_3vl_expression() {
        let statements = parse_script(
            "SELECT id, name AS n FROM t WHERE (score > 1 OR flag IS NULL) \
             AND NOT deleted ORDER BY id DESC LIMIT 3;",
        )
        .expect("parse");
        match &statements[0] {
            Statement::Select(select) => {
                assert_eq!(select.table, "t");
                assert_eq!(select.items.len(), 2);
                assert_eq!(select.order_by.len(), 1);
                assert_eq!(select.limit, Some(3));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_group_by_and_having() {
        let statements = parse_script(
            "SELECT score, COUNT(*) AS n FROM t WHERE id > 0 \
             GROUP BY score, flag HAVING COUNT(*) > 1 ORDER BY score DESC LIMIT 2;",
        )
        .expect("parse");
        match &statements[0] {
            Statement::Select(select) => {
                assert_eq!(select.group_by.len(), 2);
                assert!(select.having.is_some());
                assert_eq!(select.order_by.len(), 1);
                assert_eq!(select.limit, Some(2));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_inner_join_and_qualified_columns() {
        let statements = parse_script(
            "SELECT u.id, d.name AS dept FROM users AS u \
             INNER JOIN depts d ON u.dept = d.id WHERE u.id > 0 ORDER BY u.id;",
        )
        .expect("parse");
        match &statements[0] {
            Statement::Select(select) => {
                assert_eq!(select.table, "users");
                assert_eq!(select.table_alias.as_deref(), Some("u"));
                assert_eq!(select.joins.len(), 1);
                assert_eq!(select.joins[0].kind, JoinKind::Inner);
                assert_eq!(select.joins[0].table, "depts");
                assert_eq!(select.joins[0].alias.as_deref(), Some("d"));
                assert_eq!(select.order_by.len(), 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_left_and_left_outer_join() {
        for sql in [
            "SELECT u.id FROM users u LEFT JOIN depts d ON u.dept = d.id;",
            "SELECT u.id FROM users u LEFT OUTER JOIN depts d ON u.dept = d.id;",
        ] {
            let statements = parse_script(sql).expect("parse left join");
            match &statements[0] {
                Statement::Select(select) => {
                    assert_eq!(select.joins.len(), 1);
                    assert_eq!(select.joins[0].kind, JoinKind::Left);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn parses_right_full_and_outer_join() {
        for (sql, expected) in [
            (
                "SELECT u.id FROM users u RIGHT JOIN depts d ON u.dept = d.id;",
                JoinKind::Right,
            ),
            (
                "SELECT u.id FROM users u RIGHT OUTER JOIN depts d ON u.dept = d.id;",
                JoinKind::Right,
            ),
            (
                "SELECT u.id FROM users u FULL JOIN depts d ON u.dept = d.id;",
                JoinKind::Full,
            ),
            (
                "SELECT u.id FROM users u FULL OUTER JOIN depts d ON u.dept = d.id;",
                JoinKind::Full,
            ),
        ] {
            let statements = parse_script(sql).expect("parse outer join");
            match &statements[0] {
                Statement::Select(select) => {
                    assert_eq!(select.joins.len(), 1);
                    assert_eq!(select.joins[0].kind, expected, "{sql}");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn parses_cross_join_and_comma_from() {
        for (sql, expected_joins) in [
            ("SELECT * FROM a CROSS JOIN b;", 1),
            ("SELECT * FROM a CROSS JOIN b AS bb;", 1),
            ("SELECT * FROM a, b;", 1),
            ("SELECT * FROM a AS x, b AS y, c;", 2),
            ("SELECT * FROM a, b CROSS JOIN c;", 2),
            ("SELECT * FROM a CROSS JOIN b, c;", 2),
        ] {
            let statements = parse_script(sql).expect("parse cross join");
            match &statements[0] {
                Statement::Select(select) => {
                    assert_eq!(select.joins.len(), expected_joins, "{sql}");
                    for join in &select.joins {
                        assert_eq!(join.kind, JoinKind::Cross, "{sql}");
                        assert_eq!(join.condition, JoinCondition::None, "{sql}");
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_on_clause_for_cross_join() {
        let error = parse_script("SELECT * FROM a CROSS JOIN b ON a.id = b.id;")
            .expect_err("cross join with ON");
        assert!(
            matches!(error, SqlError::Unsupported(_)),
            "unexpected {error:?}"
        );
    }

    #[test]
    fn parses_natural_joins() {
        for (sql, expected) in [
            ("SELECT * FROM a NATURAL JOIN b;", JoinKind::Inner),
            ("SELECT * FROM a NATURAL INNER JOIN b;", JoinKind::Inner),
            ("SELECT * FROM a NATURAL LEFT JOIN b;", JoinKind::Left),
            ("SELECT * FROM a NATURAL LEFT OUTER JOIN b;", JoinKind::Left),
            ("SELECT * FROM a NATURAL RIGHT JOIN b;", JoinKind::Right),
            ("SELECT * FROM a NATURAL FULL OUTER JOIN b;", JoinKind::Full),
        ] {
            let statements = parse_script(sql).expect("parse natural join");
            match &statements[0] {
                Statement::Select(select) => {
                    assert_eq!(select.joins.len(), 1, "{sql}");
                    assert_eq!(select.joins[0].kind, expected, "{sql}");
                    assert_eq!(select.joins[0].condition, JoinCondition::Natural, "{sql}");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn parses_using_joins() {
        for (sql, expected_kind, expected_columns) in [
            (
                "SELECT * FROM a JOIN b USING (id);",
                JoinKind::Inner,
                vec!["id"],
            ),
            (
                "SELECT * FROM a LEFT JOIN b USING (id, code);",
                JoinKind::Left,
                vec!["id", "code"],
            ),
            (
                "SELECT * FROM a FULL OUTER JOIN b USING (id);",
                JoinKind::Full,
                vec!["id"],
            ),
        ] {
            let statements = parse_script(sql).expect("parse using join");
            match &statements[0] {
                Statement::Select(select) => {
                    assert_eq!(select.joins.len(), 1, "{sql}");
                    assert_eq!(select.joins[0].kind, expected_kind, "{sql}");
                    assert_eq!(
                        select.joins[0].condition,
                        JoinCondition::Using(
                            expected_columns.iter().map(ToString::to_string).collect()
                        ),
                        "{sql}"
                    );
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_conflicting_join_conditions() {
        for sql in [
            "SELECT * FROM a NATURAL JOIN b ON a.id = b.id;",
            "SELECT * FROM a NATURAL JOIN b USING (id);",
            "SELECT * FROM a NATURAL CROSS JOIN b;",
            "SELECT * FROM a CROSS JOIN b USING (id);",
            "SELECT * FROM a JOIN b USING (id) ON a.id = b.id;",
            "SELECT * FROM a OUTER JOIN b ON a.id = b.id;",
            "SELECT * FROM a, b OUTER JOIN c ON b.id = c.id;",
        ] {
            let error = parse_script(sql).expect_err("conflicting join");
            assert!(
                matches!(error, SqlError::Unsupported(_) | SqlError::Parse { .. }),
                "{sql} returned {error:?}"
            );
        }
    }

    #[test]
    fn parses_searched_and_simple_case() {
        let statements =
            parse_script("SELECT CASE WHEN flag THEN 1 WHEN id > 1 THEN 2 ELSE 3 END AS c FROM t;")
                .expect("parse searched case");
        let Statement::Select(select) = &statements[0] else {
            panic!("expected select");
        };
        let SelectItem::Expr { expr, alias } = &select.items[0] else {
            panic!("expected expression");
        };
        assert_eq!(alias.as_deref(), Some("c"));
        match expr {
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                assert!(operand.is_none());
                assert_eq!(branches.len(), 2);
                assert!(else_expr.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }

        let statements = parse_script("SELECT CASE id WHEN 1 THEN 'a' ELSE 'b' END AS c FROM t;")
            .expect("parse simple case");
        let Statement::Select(select) = &statements[0] else {
            panic!("expected select");
        };
        let SelectItem::Expr { expr, .. } = &select.items[0] else {
            panic!("expected expression");
        };
        match expr {
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                assert!(operand.is_some());
                assert_eq!(branches.len(), 1);
                assert!(else_expr.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }

        let statements = parse_script(
            "SELECT CASE WHEN id = 1 THEN CASE WHEN flag THEN 2 END ELSE 3 END AS c FROM t;",
        )
        .expect("parse nested case");
        let Statement::Select(select) = &statements[0] else {
            panic!("expected select");
        };
        let SelectItem::Expr { expr, .. } = &select.items[0] else {
            panic!("expected expression");
        };
        assert!(matches!(expr, Expr::Case { branches, .. } if branches.len() == 1));

        assert!(parse_script("SELECT CASE WHEN flag THEN 1 FROM t;").is_err());
        assert!(parse_script("SELECT CASE WHEN flag 1 END FROM t;").is_err());
        assert!(parse_script("SELECT CASE END FROM t;").is_err());
    }

    #[test]
    fn rejects_excessive_expression_nesting() {
        let depth = MAX_EXPR_DEPTH + 16;
        let nested = format!("SELECT {}1{} FROM t;", "(".repeat(depth), ")".repeat(depth));
        let error = parse_script(&nested).expect_err("nested parentheses must be rejected");
        assert!(
            matches!(error, SqlError::Parse { ref message, .. } if message.contains("too deep")),
            "{error:?}"
        );

        let nots = format!("SELECT {}1 FROM t;", "NOT ".repeat(depth));
        let error = parse_script(&nots).expect_err("NOT chain must be rejected");
        assert!(
            matches!(error, SqlError::Parse { ref message, .. } if message.contains("too deep")),
            "{error:?}"
        );

        let negatives = format!("SELECT {}1 FROM t;", "- ".repeat(depth));
        let error = parse_script(&negatives).expect_err("unary minus chain must be rejected");
        assert!(
            matches!(error, SqlError::Parse { ref message, .. } if message.contains("too deep")),
            "{error:?}"
        );

        let nested = format!("SELECT {}1{} FROM t;", "(".repeat(32), ")".repeat(32));
        parse_script(&nested).expect("modest nesting must still parse");
    }

    #[test]
    fn rejects_bad_syntax() {
        assert!(parse_script("SELECT FROM t;").is_err());
        assert!(parse_script("CREATE TABLE t (a);").is_err());
        assert!(parse_script("INSERT INTO t VALUES (1,);").is_err());
    }

    #[test]
    fn parses_with_recursive_cte() {
        let statements = parse_script(
            "WITH RECURSIVE reach(node) AS (SELECT src FROM edges WHERE src = 0 \
             UNION SELECT e.dst FROM edges e JOIN reach r ON e.src = r.node) \
             SELECT node FROM reach",
        )
        .expect("parse");
        let Statement::With(with) = &statements[0] else {
            panic!("expected WITH");
        };
        assert!(with.recursive);
        assert_eq!(with.ctes.len(), 1);
        assert_eq!(with.ctes[0].name, "reach");
        assert_eq!(with.ctes[0].columns, vec!["node".to_string()]);
        assert!(!with.ctes[0].union_all);
        let Query::Select(seed) = &*with.ctes[0].seed else {
            panic!("expected SELECT seed");
        };
        assert_eq!(seed.table, "edges");
        let Query::Select(step) = &**with.ctes[0].step.as_ref().expect("step") else {
            panic!("expected SELECT step");
        };
        assert_eq!(step.table, "edges");
        let Statement::Select(outer) = &*with.body else {
            panic!("expected SELECT body");
        };
        assert_eq!(outer.table, "reach");
    }

    #[test]
    fn parses_union_all_and_multiple_ctes() {
        let statements = parse_script(
            "WITH RECURSIVE a(x) AS (SELECT id FROM t UNION ALL SELECT x FROM a), \
             b(y) AS (SELECT x FROM a UNION SELECT y FROM b) SELECT y FROM b",
        )
        .expect("parse");
        let Statement::With(with) = &statements[0] else {
            panic!("expected WITH");
        };
        assert_eq!(with.ctes.len(), 2);
        assert!(with.ctes[0].union_all);
        assert!(!with.ctes[1].union_all);
        let Query::Select(seed) = &*with.ctes[1].seed else {
            panic!("expected SELECT seed");
        };
        assert_eq!(seed.table, "a");
        let Statement::Select(outer) = &*with.body else {
            panic!("expected SELECT body");
        };
        assert_eq!(outer.table, "b");
    }
}
