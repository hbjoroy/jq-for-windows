use crate::{BinaryOp, Builtin, Filter, Format, InterpolationPart, ParseError, UnaryOp, UpdateOp};
use serde_json::Value;

type Error = ParseError;

pub(crate) fn compile(source: &str) -> Result<Filter, ParseError> {
    let mut parser = Parser { source, offset: 0 };
    let filter = parser.parse_program()?;
    parser.whitespace();
    if parser.offset != source.len() {
        return Err(ParseError::unexpected(parser.offset));
    }
    Ok(filter)
}
struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> Result<Filter, Error> {
        if !self.eat_word("def") {
            return self.parse_comma();
        }
        let name = self.identifier_required()?;
        let parameters = if self.eat('(') {
            let mut parameters = Vec::new();
            if !self.eat(')') {
                loop {
                    parameters.push(self.identifier_required()?);
                    if self.eat(')') {
                        break;
                    }
                    self.expect(';')?;
                }
            }
            parameters
        } else {
            Vec::new()
        };
        self.expect(':')?;
        let body = self.parse_comma()?;
        self.expect(';')?;
        let then = self.parse_program()?;
        Ok(Filter::Define {
            name,
            parameters,
            body: Box::new(body),
            then: Box::new(then),
        })
    }

    fn parse_comma(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_pipe()?;
        while self.eat(',') {
            filter = Filter::Comma(Box::new(filter), Box::new(self.parse_pipe()?));
        }
        Ok(filter)
    }

    fn parse_assignment(&mut self) -> Result<Filter, Error> {
        let target = self.parse_alternative()?;
        let operation = if self.eat_str("|=") {
            Some(UpdateOp::Modify)
        } else if self.eat_str("+=") {
            Some(UpdateOp::Add)
        } else if self.eat_str("-=") {
            Some(UpdateOp::Subtract)
        } else if self.eat_str("*=") {
            Some(UpdateOp::Multiply)
        } else if self.eat_str("/=") {
            Some(UpdateOp::Divide)
        } else if self.eat_str("%=") {
            Some(UpdateOp::Remainder)
        } else if !self.starts_with("==") && self.eat('=') {
            Some(UpdateOp::Assign)
        } else {
            None
        };
        match operation {
            Some(operation) => Ok(Filter::Update {
                target: Box::new(target),
                operation,
                value: Box::new(self.parse_assignment()?),
            }),
            None => Ok(target),
        }
    }

    fn parse_pipe(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_assignment()?;
        loop {
            if self.eat_word("as") {
                self.expect('$')?;
                let name = self.identifier_required()?;
                self.expect('|')?;
                return Ok(Filter::Bind {
                    source: Box::new(filter),
                    name,
                    body: Box::new(self.parse_pipe()?),
                });
            }
            if self.starts_with("|=") {
                break;
            } else if self.eat('|') {
                filter = Filter::Pipe(Box::new(filter), Box::new(self.parse_assignment()?));
            } else {
                break;
            }
        }
        Ok(filter)
    }

    fn parse_alternative(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_or()?;
        while self.eat_str("//") {
            filter = Filter::Binary(
                BinaryOp::Alternative,
                Box::new(filter),
                Box::new(self.parse_or()?),
            );
        }
        Ok(filter)
    }

    fn parse_or(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_and()?;
        while self.eat_word("or") {
            filter = Filter::Binary(BinaryOp::Or, Box::new(filter), Box::new(self.parse_and()?));
        }
        Ok(filter)
    }

    fn parse_and(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_comparison()?;
        while self.eat_word("and") {
            filter = Filter::Binary(
                BinaryOp::And,
                Box::new(filter),
                Box::new(self.parse_comparison()?),
            );
        }
        Ok(filter)
    }

    fn parse_comparison(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_additive()?;
        loop {
            let operator = if self.eat_str("==") {
                Some(BinaryOp::Equal)
            } else if self.eat_str("!=") {
                Some(BinaryOp::NotEqual)
            } else if self.eat_str("<=") {
                Some(BinaryOp::LessEqual)
            } else if self.eat_str(">=") {
                Some(BinaryOp::GreaterEqual)
            } else if self.eat('<') {
                Some(BinaryOp::Less)
            } else if self.eat('>') {
                Some(BinaryOp::Greater)
            } else {
                None
            };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(operator, Box::new(filter), Box::new(self.parse_additive()?));
        }
        Ok(filter)
    }

    fn parse_additive(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_multiplicative()?;
        loop {
            let operator = if self.starts_with("+=") || self.starts_with("-=") {
                None
            } else if self.eat('+') {
                Some(BinaryOp::Add)
            } else if self.eat('-') {
                Some(BinaryOp::Subtract)
            } else {
                None
            };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(
                operator,
                Box::new(filter),
                Box::new(self.parse_multiplicative()?),
            );
        }
        Ok(filter)
    }

    fn parse_multiplicative(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_unary()?;
        loop {
            let operator =
                if self.starts_with("*=") || self.starts_with("/=") || self.starts_with("%=") {
                    None
                } else if self.eat('*') {
                    Some(BinaryOp::Multiply)
                } else if self.starts_with("//") {
                    None
                } else if self.eat('/') {
                    Some(BinaryOp::Divide)
                } else if self.eat('%') {
                    Some(BinaryOp::Remainder)
                } else {
                    None
                };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(operator, Box::new(filter), Box::new(self.parse_unary()?));
        }
        Ok(filter)
    }

    fn parse_unary(&mut self) -> Result<Filter, Error> {
        if self.eat_word("not") {
            Ok(Filter::Unary(UnaryOp::Not, Box::new(self.parse_unary()?)))
        } else if self.eat('-') {
            Ok(Filter::Unary(
                UnaryOp::Negate,
                Box::new(self.parse_unary()?),
            ))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Filter, Error> {
        self.whitespace();
        let mut filter = match self.peek() {
            Some('.') => self.parse_dot()?,
            Some('[') => self.parse_array()?,
            Some('{') => self.parse_object()?,
            Some('$') => {
                self.bump();
                Filter::Variable(self.identifier_required()?)
            }
            Some('(') => {
                self.bump();
                let inner = self.parse_program()?;
                self.expect(')')?;
                inner
            }
            Some('"') => self.parse_interpolated_string()?,
            Some('@') => self.parse_format()?,
            Some('-' | '0'..='9') => Filter::Literal(self.json_literal()?),
            Some(c) if is_identifier_start(c) => self.parse_named_or_control()?,
            _ => return Err(Error::new("expected a filter", self.offset)),
        };

        loop {
            if self.eat('?') {
                filter = Filter::Optional(Box::new(filter));
            } else if self.next_non_whitespace() == Some('[') {
                let access = self.parse_bracket_access()?;
                filter = Filter::Pipe(Box::new(filter), Box::new(access));
            } else if self.next_non_whitespace() == Some('.') {
                self.eat('.');
                let name = self.identifier_required()?;
                filter = Filter::Pipe(Box::new(filter), Box::new(Filter::Field(name)));
            } else {
                break;
            }
        }
        Ok(filter)
    }

    fn parse_dot(&mut self) -> Result<Filter, Error> {
        self.expect('.')?;
        if self.next_non_whitespace() == Some('[') {
            self.parse_bracket_access()
        } else if self.next_non_whitespace().is_some_and(is_identifier_start) {
            Ok(Filter::Field(self.identifier()))
        } else {
            Ok(Filter::Identity)
        }
    }

    fn parse_interpolated_string(&mut self) -> Result<Filter, Error> {
        let start = self.offset;
        self.expect('"')?;
        let mut raw = String::new();
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.bump();
                    if !raw.is_empty() || parts.is_empty() {
                        parts.push(InterpolationPart::Text(decode_string_fragment(
                            &raw, start,
                        )?));
                    }
                    return Ok(Filter::Interpolate(parts));
                }
                Some('\\') => {
                    self.bump();
                    if self.peek() == Some('(') {
                        self.bump();
                        if !raw.is_empty() {
                            parts.push(InterpolationPart::Text(decode_string_fragment(
                                &raw, start,
                            )?));
                            raw.clear();
                        }
                        parts.push(InterpolationPart::Filter(self.parse_comma()?));
                        self.expect(')')?;
                    } else {
                        raw.push('\\');
                        let Some(escaped) = self.peek() else {
                            return Err(ParseError::invalid_literal("unterminated escape", start));
                        };
                        raw.push(escaped);
                        self.bump();
                    }
                }
                Some(character) => {
                    raw.push(character);
                    self.bump();
                }
                None => return Err(ParseError::invalid_literal("unterminated string", start)),
            }
        }
    }

    fn parse_format(&mut self) -> Result<Filter, Error> {
        self.expect('@')?;
        let name = self.identifier_required()?;
        let format = match name.as_str() {
            "json" => Format::Json,
            "csv" => Format::Csv,
            "tsv" => Format::Tsv,
            "uri" => Format::Uri,
            "base64" => Format::Base64,
            _ => return Err(ParseError::expected("a known format filter", self.offset)),
        };
        Ok(Filter::Builtin(Builtin::Format(format), Vec::new()))
    }

    fn parse_named_or_control(&mut self) -> Result<Filter, Error> {
        if self.eat_word("if") {
            self.parse_if_tail()
        } else if self.eat_word("try") {
            self.parse_try()
        } else if self.eat_word("reduce") {
            self.parse_reduce()
        } else if self.eat_word("foreach") {
            self.parse_foreach()
        } else {
            self.parse_named_filter()
        }
    }

    fn parse_if_tail(&mut self) -> Result<Filter, Error> {
        let condition = self.parse_comma()?;
        self.expect_word("then")?;
        let then_branch = self.parse_comma()?;
        let else_branch = if self.eat_word("elif") {
            self.parse_if_tail()?
        } else {
            self.expect_word("else")?;
            let branch = self.parse_comma()?;
            self.expect_word("end")?;
            branch
        };
        Ok(Filter::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
        })
    }

    fn parse_try(&mut self) -> Result<Filter, Error> {
        let body = self.parse_comma()?;
        let handler = if self.eat_word("catch") {
            Some(Box::new(self.parse_comma()?))
        } else {
            None
        };
        Ok(Filter::Try {
            body: Box::new(body),
            handler,
        })
    }

    fn parse_reduce(&mut self) -> Result<Filter, Error> {
        let source = self.parse_alternative()?;
        self.expect_word("as")?;
        self.expect('$')?;
        let variable = self.identifier_required()?;
        self.expect('(')?;
        let initial = self.parse_comma()?;
        self.expect(';')?;
        let update = self.parse_comma()?;
        self.expect(')')?;
        Ok(Filter::Reduce {
            source: Box::new(source),
            variable,
            initial: Box::new(initial),
            update: Box::new(update),
        })
    }

    fn parse_foreach(&mut self) -> Result<Filter, Error> {
        let source = self.parse_alternative()?;
        self.expect_word("as")?;
        self.expect('$')?;
        let variable = self.identifier_required()?;
        self.expect('(')?;
        let initial = self.parse_comma()?;
        self.expect(';')?;
        let update = self.parse_comma()?;
        let extract = if self.eat(';') {
            self.parse_comma()?
        } else {
            Filter::Identity
        };
        self.expect(')')?;
        Ok(Filter::Foreach {
            source: Box::new(source),
            variable,
            initial: Box::new(initial),
            update: Box::new(update),
            extract: Box::new(extract),
        })
    }

    fn parse_bracket_access(&mut self) -> Result<Filter, Error> {
        self.expect('[')?;
        if self.eat(']') {
            return Ok(Filter::Iterate);
        }
        let start = self.optional_integer()?;
        if self.eat(':') {
            let end = self.optional_integer()?;
            self.expect(']')?;
            Ok(Filter::Slice(start, end))
        } else if let Some(index) = start {
            self.expect(']')?;
            Ok(Filter::Index(index))
        } else {
            Err(Error::new("expected an index or slice", self.offset))
        }
    }

    fn parse_array(&mut self) -> Result<Filter, Error> {
        self.expect('[')?;
        if self.eat(']') {
            return Ok(Filter::Literal(Value::Array(Vec::new())));
        }
        let inner = self.parse_comma()?;
        self.expect(']')?;
        Ok(Filter::Array(Box::new(inner)))
    }

    fn parse_object(&mut self) -> Result<Filter, Error> {
        self.expect('{')?;
        let mut fields = Vec::new();
        if self.eat('}') {
            return Ok(Filter::Object(fields));
        }
        loop {
            let name = if self.next_non_whitespace() == Some('"') {
                self.json_literal()?.as_str().unwrap().to_owned()
            } else {
                self.identifier_required()?
            };
            self.expect(':')?;
            fields.push((name, self.parse_pipe()?));
            if !self.eat(',') {
                break;
            }
        }
        self.expect('}')?;
        Ok(Filter::Object(fields))
    }

    fn parse_named_filter(&mut self) -> Result<Filter, Error> {
        let word = self.identifier();
        match word.as_str() {
            "null" => Ok(Filter::Literal(Value::Null)),
            "true" => Ok(Filter::Literal(Value::Bool(true))),
            "false" => Ok(Filter::Literal(Value::Bool(false))),
            "length" => Ok(Filter::Builtin(Builtin::Length, Vec::new())),
            "type" => Ok(Filter::Builtin(Builtin::Type, Vec::new())),
            "keys" => Ok(Filter::Builtin(Builtin::Keys, Vec::new())),
            "empty" => Ok(Filter::Builtin(Builtin::Empty, Vec::new())),
            "tostring" => Ok(Filter::Builtin(Builtin::ToString, Vec::new())),
            "paths" => Ok(Filter::Builtin(Builtin::Paths, Vec::new())),
            "sort" => Ok(Filter::Builtin(Builtin::Sort, Vec::new())),
            "unique" => Ok(Filter::Builtin(Builtin::Unique, Vec::new())),
            "min" => Ok(Filter::Builtin(Builtin::Min, Vec::new())),
            "max" => Ok(Filter::Builtin(Builtin::Max, Vec::new())),
            "add" if self.next_non_whitespace() == Some('(') => Ok(Filter::Call {
                name: word,
                arguments: self.parse_call_arguments()?,
            }),
            "add" => Ok(Filter::Builtin(Builtin::Add, Vec::new())),
            "tonumber" => Ok(Filter::Builtin(Builtin::ToNumber, Vec::new())),
            "fromjson" => Ok(Filter::Builtin(Builtin::FromJson, Vec::new())),
            "flatten" if self.next_non_whitespace() == Some('(') => {
                self.parse_builtin_arguments(Builtin::Flatten, 1)
            }
            "flatten" => Ok(Filter::Builtin(Builtin::Flatten, Vec::new())),
            "has" => self.parse_builtin_argument(Builtin::Has),
            "map" => self.parse_builtin_argument(Builtin::Map),
            "select" => self.parse_builtin_argument(Builtin::Select),
            "sort_by" => self.parse_builtin_arguments(Builtin::SortBy, 1),
            "group_by" => self.parse_builtin_arguments(Builtin::GroupBy, 1),
            "unique_by" => self.parse_builtin_arguments(Builtin::UniqueBy, 1),
            "contains" => self.parse_builtin_arguments(Builtin::Contains, 1),
            "inside" => self.parse_builtin_arguments(Builtin::Inside, 1),
            "startswith" => self.parse_builtin_arguments(Builtin::StartsWith, 1),
            "endswith" => self.parse_builtin_arguments(Builtin::EndsWith, 1),
            "split" => self.parse_builtin_arguments(Builtin::Split, 1),
            "join" => self.parse_builtin_arguments(Builtin::Join, 1),
            "test" => self.parse_builtin_argument_range(Builtin::Test, 1, 2),
            "match" => self.parse_builtin_argument_range(Builtin::Match, 1, 2),
            "capture" => self.parse_builtin_argument_range(Builtin::Capture, 1, 2),
            "scan" => self.parse_builtin_argument_range(Builtin::Scan, 1, 2),
            "sub" => self.parse_builtin_arguments(Builtin::Sub, 2),
            "gsub" => self.parse_builtin_arguments(Builtin::Gsub, 2),
            "del" => self.parse_builtin_arguments(Builtin::Del, 1),
            "getpath" => self.parse_builtin_arguments(Builtin::GetPath, 1),
            "setpath" => self.parse_builtin_arguments(Builtin::SetPath, 2),
            "error" if self.next_non_whitespace() == Some('(') => {
                self.parse_builtin_argument(Builtin::Error)
            }
            "error" => Ok(Filter::Builtin(Builtin::Error, Vec::new())),
            _ if self.next_non_whitespace() == Some('(') => Ok(Filter::Call {
                name: word,
                arguments: self.parse_call_arguments()?,
            }),
            _ => Ok(Filter::Call {
                name: word,
                arguments: Vec::new(),
            }),
        }
    }

    fn parse_call_arguments(&mut self) -> Result<Vec<Filter>, Error> {
        self.expect('(')?;
        let mut arguments = Vec::new();
        if self.eat(')') {
            return Ok(arguments);
        }
        loop {
            arguments.push(self.parse_comma()?);
            if self.eat(')') {
                break;
            }
            self.expect(';')?;
        }
        Ok(arguments)
    }

    fn parse_builtin_argument(&mut self, builtin: Builtin) -> Result<Filter, Error> {
        self.expect('(')?;
        let argument = self.parse_comma()?;
        self.expect(')')?;
        Ok(Filter::Builtin(builtin, vec![argument]))
    }

    fn parse_builtin_arguments(
        &mut self,
        builtin: Builtin,
        expected: usize,
    ) -> Result<Filter, Error> {
        let arguments = self.parse_call_arguments()?;
        if arguments.len() != expected {
            return Err(ParseError::expected(
                format!("{expected} function arguments"),
                self.offset,
            ));
        }
        Ok(Filter::Builtin(builtin, arguments))
    }

    fn parse_builtin_argument_range(
        &mut self,
        builtin: Builtin,
        minimum: usize,
        maximum: usize,
    ) -> Result<Filter, Error> {
        let arguments = self.parse_call_arguments()?;
        if !(minimum..=maximum).contains(&arguments.len()) {
            return Err(ParseError::expected(
                format!("{minimum} to {maximum} function arguments"),
                self.offset,
            ));
        }
        Ok(Filter::Builtin(builtin, arguments))
    }

    fn json_literal(&mut self) -> Result<Value, Error> {
        self.whitespace();
        let start = self.offset;
        if self.peek() == Some('"') {
            self.bump();
            let mut escaped = false;
            loop {
                match self.peek() {
                    Some('"') if !escaped => {
                        self.bump();
                        break;
                    }
                    Some('\\') if !escaped => {
                        escaped = true;
                        self.bump();
                    }
                    Some(_) => {
                        escaped = false;
                        self.bump();
                    }
                    None => return Err(ParseError::invalid_literal("unterminated string", start)),
                }
            }
        } else {
            if self.peek() == Some('-') {
                self.bump();
            }
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                self.bump();
            }
        }
        serde_json::from_str(&self.source[start..self.offset])
            .map_err(|error| ParseError::invalid_literal(error.to_string(), start))
    }

    fn optional_integer(&mut self) -> Result<Option<i64>, Error> {
        self.whitespace();
        let start = self.offset;
        if self.peek() == Some('-') {
            self.bump();
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.offset == start {
            return Ok(None);
        }
        if &self.source[start..self.offset] == "-" {
            return Err(Error::new("expected an integer", start));
        }
        self.source[start..self.offset]
            .parse()
            .map(Some)
            .map_err(|_| Error::new("invalid integer", start))
    }

    fn identifier_required(&mut self) -> Result<String, Error> {
        self.whitespace();
        if self.peek().is_some_and(is_identifier_start) {
            Ok(self.identifier())
        } else {
            Err(Error::new("expected an identifier", self.offset))
        }
    }

    fn identifier(&mut self) -> String {
        self.whitespace();
        let start = self.offset;
        self.bump();
        while self.peek().is_some_and(is_identifier_continue) {
            self.bump();
        }
        self.source[start..self.offset].to_owned()
    }

    fn expect(&mut self, expected: char) -> Result<(), Error> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(Error::new(format!("expected '{expected}'"), self.offset))
        }
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn next_non_whitespace(&mut self) -> Option<char> {
        self.whitespace();
        self.peek()
    }

    fn eat(&mut self, expected: char) -> bool {
        self.whitespace();
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_str(&mut self, expected: &str) -> bool {
        self.whitespace();
        if self.source[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn starts_with(&mut self, expected: &str) -> bool {
        self.whitespace();
        self.source[self.offset..].starts_with(expected)
    }

    fn eat_word(&mut self, expected: &str) -> bool {
        self.whitespace();
        let rest = &self.source[self.offset..];
        if rest.starts_with(expected)
            && rest[expected.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_identifier_continue(c))
        {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), Error> {
        if self.eat_word(expected) {
            Ok(())
        } else {
            Err(ParseError::expected(expected, self.offset))
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.offset += c.len_utf8();
        }
    }
}

fn is_identifier_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_identifier_continue(c: char) -> bool {
    is_identifier_start(c) || c.is_ascii_digit()
}

fn decode_string_fragment(raw: &str, offset: usize) -> Result<String, ParseError> {
    serde_json::from_str::<String>(&format!("\"{raw}\""))
        .map_err(|error| ParseError::invalid_literal(error.to_string(), offset))
}
