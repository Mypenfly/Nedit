//!@Open:./tests/data/rust_parser.rs
//!@Location:
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
		//!@New:
		nothing:String,
		...
//!@Off:Location
//!@Off:Open
