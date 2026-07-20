use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Identity,
    Literal(Value),
    Field(String),
    Index(i64),
    Iterate,
    Slice(Option<i64>, Option<i64>),
    Array(Box<Filter>),
    Object(Vec<(String, Filter)>),
    Pipe(Box<Filter>, Box<Filter>),
    Comma(Box<Filter>, Box<Filter>),
    Optional(Box<Filter>),
    Unary(UnaryOp, Box<Filter>),
    Binary(BinaryOp, Box<Filter>, Box<Filter>),
    Builtin(Builtin, Vec<Filter>),
    Variable(String),
    Bind {
        source: Box<Filter>,
        name: String,
        body: Box<Filter>,
    },
    Define {
        name: String,
        parameters: Vec<String>,
        body: Box<Filter>,
        then: Box<Filter>,
    },
    Call {
        name: String,
        arguments: Vec<Filter>,
    },
    If {
        condition: Box<Filter>,
        then_branch: Box<Filter>,
        else_branch: Box<Filter>,
    },
    Try {
        body: Box<Filter>,
        handler: Option<Box<Filter>>,
    },
    Reduce {
        source: Box<Filter>,
        variable: String,
        initial: Box<Filter>,
        update: Box<Filter>,
    },
    Foreach {
        source: Box<Filter>,
        variable: String,
        initial: Box<Filter>,
        update: Box<Filter>,
        extract: Box<Filter>,
    },
    Update {
        target: Box<Filter>,
        operation: UpdateOp,
        value: Box<Filter>,
    },
    Interpolate(Vec<InterpolationPart>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Length,
    Type,
    Keys,
    Has,
    Map,
    Select,
    Empty,
    Error,
    ToString,
    Del,
    GetPath,
    SetPath,
    Paths,
    Sort,
    SortBy,
    GroupBy,
    Unique,
    UniqueBy,
    Min,
    Max,
    Add,
    Flatten,
    Contains,
    Inside,
    StartsWith,
    EndsWith,
    Split,
    Join,
    ToNumber,
    FromJson,
    Test,
    Match,
    Capture,
    Scan,
    Sub,
    Gsub,
    Format(Format),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpolationPart {
    Text(String),
    Filter(Filter),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Csv,
    Tsv,
    Uri,
    Base64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp {
    Assign,
    Modify,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Alternative,
}
