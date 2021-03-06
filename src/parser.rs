use lexicon::Token;
use lexicon::Token::*;
use tokenizer::Tokenizer;
use std::iter::Peekable;
use grammar::*;
use grammar::Statement::*;
use grammar::Expression::*;
use grammar::ClassMember::*;
use grammar::OperatorType::*;

/// If the next token matches `$p`, consume that token and return
/// true, else do nothing and return false
macro_rules! allow {
    ($parser:ident, $p:pat) => {
        match $parser.lookahead() {
            Some(&$p) => {
                $parser.consume();
                true
            },
            _ => false
        }
    };
    {$parser:ident $( $p:pat => $then:expr ),* } => ({
        match $parser.lookahead() {
            $(
                Some(&$p) => {
                    $parser.consume();
                    $then;
                }
            )*
            _ => {}
        }
    });
}

/// Expects next token to match `$p`, otherwise panics.
macro_rules! expect {
    ($parser:ident, $p:pat => $value:ident) => (
        match $parser.consume() {
            Some($p) => $value,
            None     => panic!("Unexpected end of program"),
            token    => unexpected_token!($parser, token),
        }
    );
    ($parser:ident, $p:pat) => (
        match $parser.consume() {
            Some($p) => {},
            None     => panic!("Unexpected end of program"),
            token    => unexpected_token!($parser, token),
        }
    )
}

macro_rules! unexpected_token {
    ($parser:ident) => ({
        if let Some(token) = $parser.consume() {
            unexpected_token!($parser, token);
        } else {
            panic!("Unexpected end of program");
        }
    });
    ($parser:ident, $token:expr) => {
        panic!("Unexpected token {:?}", $token)
    }
}

/// Evaluates the `$eval` expression, then expects a semicolon or
/// end of program. If neither is found, but a LineTermination
/// occured on previous token, parsing will continue as if a
/// semicolon was present. In other cases cause a panic.
macro_rules! statement {
    ($parser:ident, $eval:expr) => ({
        let value = $eval;

        let is_end = match $parser.lookahead() {
            Some(&Semicolon) => {
                $parser.consume();
                true
            },
            None => true,
            _    => false
        };

        if !is_end && !$parser.allow_asi {
            unexpected_token!($parser);
        };

        value
    })
}

/// Read a list of items with predefined `$start`, `$end` and
/// `$separator` tokens and an `$item` expression that is then
/// pushed onto a vector.
macro_rules! list {
    ($parser:ident, $item:expr, $start:pat, $end:pat) => ({
        expect!($parser, $start);

        let mut list = Vec::new();
        loop {
            if allow!($parser, $end) {
                break;
            }
            list.push($item);

            match $parser.consume() {
                Some(Comma) => allow!{ $parser $end => break },
                Some($end)  => break,
                _           => {},
            }
        }

        list
    });
    ($parser:ident ( $item:expr )) => {
        list!($parser, $item, ParenOn, ParenOff)
    };
    ($parser:ident [ $item:expr ]) => {
        list!($parser, $item, BracketOn, BracketOff)
    };
    ($parser:ident { $item:expr }) => {
        list!($parser, $item, BlockOn, BlockOff)
    };
}

macro_rules! surround {
    ($parser:ident ( $eval:expr )) => ({
        expect!($parser, ParenOn);
        let value = $eval;
        expect!($parser, ParenOff);
        value
    });
    ($parser:ident [ $eval:expr ]) => ({
        expect!($parser, BracketOn);
        let value = $eval;
        expect!($parser, BracketOff);
        value
    });
}

pub struct Parser<'a> {
    tokenizer: Peekable<Tokenizer<'a>>,
    allow_asi: bool,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a String) -> Self {
        Parser {
            tokenizer: Tokenizer::new(source).peekable(),
            allow_asi: false,
        }
    }

    #[inline(always)]
    fn handle_line_termination(&mut self) {
        while let Some(&LineTermination) = self.tokenizer.peek() {
            self.tokenizer.next();
            self.allow_asi = true;
        }
    }

    #[inline(always)]
    fn consume(&mut self) -> Option<Token> {
        self.handle_line_termination();
        let token = self.tokenizer.next();

        // println!("Consume {:?}", token);

        self.allow_asi = false;
        token
    }

    #[inline(always)]
    fn lookahead(&mut self) -> Option<&Token> {
        self.handle_line_termination();
        self.tokenizer.peek()
    }

    fn array_expression(&mut self) -> Expression {
        ArrayExpression(list!(self [ self.expression(0) ]))
    }

    #[inline(always)]
    fn object_member(&mut self) -> ObjectMember {
        match self.consume() {
            Some(Identifier(key)) | Some(Literal(LiteralString(key))) => {
                if allow!(self, Colon) {
                    ObjectMember::Literal {
                        key: key,
                        value: self.expression(0),
                    }
                } else {
                    ObjectMember::Shorthand {
                        key: key,
                    }
                }
            },
            Some(BracketOn) => {
                let key = self.expression(0);
                expect!(self, BracketOff);
                expect!(self, Colon);
                ObjectMember::Computed {
                    key: key,
                    value: self.expression(0),
                }
            },
            token => {
                panic!("Expected object key, got {:?}", token)
            }
        }
    }

    fn object_expression(&mut self) -> Expression {
        ObjectExpression(list!(self { self.object_member() }))
    }

    fn block_or_statement(&mut self) -> Statement {
        if let Some(&BlockOn) = self.lookahead() {
            BlockStatement {
                body: self.block_body()
            }
        } else {
            ExpressionStatement(self.expression(0))
        }
    }

    fn block_body(&mut self) -> Vec<Statement> {
        expect!(self, BlockOn);

        let mut body = Vec::new();
        loop {
            allow!{ self BlockOff => break };

            body.push(
                self.statement().expect("Unexpected end of statements block")
            )
        }

        body
    }

    fn arrow_function_expression(&mut self, p: Expression) -> Expression {
        let params: Vec<Parameter> = match p {
            IdentifierExpression(name) => {
                vec![Parameter { name: name }]
            },
            _ =>
                panic!("Can cast {:?} to parameters", p),
        };

        ArrowFunctionExpression {
            params: params,
            body: Box::new(self.block_or_statement())
        }
    }

    #[inline(always)]
    fn prefix_expression(&mut self) -> Expression {
        let operator = expect!(self, Operator(op) => op);
        let bp = operator.binding_power(true);

        if !operator.prefix() {
            panic!("Unexpected operator {:?}", operator);
        }

        PrefixExpression {
            operator: operator,
            operand: Box::new(self.expression(bp)),
        }
    }

    #[inline(always)]
    fn infix_expression(&mut self, left: Expression, bp: u8) -> Expression {
        let operator = expect!(self, Operator(op) => op);

        match operator {
            Increment | Decrement => PostfixExpression {
                operator: operator,
                operand: Box::new(left),
            },

            Accessor => MemberExpression {
                object: Box::new(left),
                property: Box::new(MemberKey::Literal(
                    expect!(self, Identifier(key) => key)
                )),
            },

            Conditional => ConditionalExpression {
                test: Box::new(left),
                consequent: Box::new(self.expression(bp)),
                alternate: {
                    expect!(self, Colon);
                    Box::new(self.expression(bp))
                }
            },

            FatArrow => self.arrow_function_expression(left),

            _ => {
                if !operator.infix() {
                    panic!("Unexpected operator {:?}", operator);
                }

                BinaryExpression {
                    left: Box::new(left),
                    operator: operator,
                    right: Box::new(
                        self.expression(bp)
                    )
                }
            }
        }
    }

    fn expression(&mut self, lbp: u8) -> Expression {
        let mut left = match self.lookahead() {
            Some(&Identifier(_)) => {
                IdentifierExpression(expect!(self, Identifier(v) => v))
            },
            Some(&Literal(_))    => {
                LiteralExpression(expect!(self, Literal(v) => v))
            },
            Some(&Operator(_)) => self.prefix_expression(),
            Some(&ParenOn)     => surround!(self ( self.expression(19) )),
            Some(&BracketOn)   => self.array_expression(),
            Some(&BlockOn)     => self.object_expression(),
            _                  => unexpected_token!(self)
        };

        'right: loop {
            let rbp = match self.lookahead() {
                Some(&Operator(ref op)) => op.binding_power(false),
                _                       => 0,
            };

            if lbp > rbp {
                break 'right;
            }

            left = match self.lookahead() {
                Some(&Operator(_)) => self.infix_expression(left, rbp),

                Some(&ParenOn)     => CallExpression {
                    callee: Box::new(left),
                    arguments: list!(self ( self.expression(0) ))
                },

                Some(&BracketOn)   => MemberExpression {
                    object: Box::new(left),
                    property: Box::new(MemberKey::Computed(
                        surround!(self [ self.expression(0) ])
                    ))
                },

                _                  => break 'right,
            }
        }

        left
    }

    fn variable_declaration_statement(
        &mut self, kind: VariableDeclarationKind
    ) -> Statement {
        let mut declarations = Vec::new();

        loop {
            let name = expect!(self, Identifier(name) => name);
            expect!(self, Operator(Assign));
            declarations.push((
                name,
                self.expression(0)
            ));

            allow!{ self Comma => continue };
            break;
        }

        statement!(self, VariableDeclarationStatement {
            kind: kind,
            declarations: declarations,
        })
    }

    fn expression_statement(&mut self) -> Statement {
        statement!(self, ExpressionStatement(
            self.expression(0)
        ))
    }

    fn return_statement(&mut self) -> Statement {
        statement!(self, ReturnStatement(
            self.expression(0)
        ))
    }

    fn if_statement(&mut self) -> Statement {
        let test = surround!(self ( self.expression(0) ));
        let consequent = Box::new(self.block_or_statement());
        let alternate = if allow!(self, Else) {
            if allow!(self, If) {
                Some(Box::new(self.if_statement()))
            } else {
                Some(Box::new(self.block_or_statement()))
            }
        } else {
            None
        };

        statement!(self, IfStatement {
            test: test,
            consequent: consequent,
            alternate: alternate,
        })
    }

    fn while_statement(&mut self) -> Statement {
        statement!(self, WhileStatement {
            test: surround!(self ( self.expression(0) )),
            body: Box::new(self.block_or_statement()),
        })
    }

    fn parameter(&mut self) -> Parameter {
        Parameter {
            name: expect!(self, Identifier(name) => name)
        }
    }

    fn function_statement(&mut self) -> Statement {
        FunctionStatement {
            name: expect!(self, Identifier(name) => name),
            params: list!(self ( self.parameter() )),
            body: self.block_body(),
        }
    }

    fn class_member(&mut self, name: String, is_static: bool) -> ClassMember {
        match self.lookahead() {
            Some(&ParenOn) => {
                if !is_static && name == "constructor" {
                    ClassConstructor {
                        params: list!(self ( self.parameter() )),
                        body: self.block_body(),
                    }
                } else {
                    ClassMethod {
                        is_static: is_static,
                        name: name,
                        params: list!(self ( self.parameter())),
                        body: self.block_body(),
                    }
                }
            },
            Some(&Operator(Assign)) => {
                self.consume();
                ClassProperty {
                    is_static: is_static,
                    name: name,
                    value: self.expression(0),
                }
            },
            _ => unexpected_token!(self),
        }
    }

    fn class_statement(&mut self) -> Statement {
        let name = expect!(self, Identifier(id) => id);
        let super_class = if allow!(self, Extends) {
            Some(expect!(self, Identifier(name) => name))
        } else {
            None
        };
        expect!(self, BlockOn);
        let mut members = Vec::new();
        'members: loop {
            members.push(match self.consume() {
                Some(Identifier(name)) => self.class_member(name, false),
                Some(Static)           => {
                    let name = expect!(self, Identifier(name) => name);
                    self.class_member(name, true)
                },
                Some(Semicolon)        => continue 'members,
                Some(BlockOff)         => break 'members,
                token                  => unexpected_token!(self, token)
            });
        }

        ClassStatement {
            name: name,
            extends: super_class,
            body: members,
        }
    }

    fn statement(&mut self) -> Option<Statement> {
        allow!{self
            Var       => return Some(self.variable_declaration_statement(
                VariableDeclarationKind::Var
            )),
            Let       => return Some(self.variable_declaration_statement(
                VariableDeclarationKind::Let
            )),
            Const     => return Some(self.variable_declaration_statement(
                VariableDeclarationKind::Const
            )),
            Return    => return Some(self.return_statement()),
            Function  => return Some(self.function_statement()),
            Class     => return Some(self.class_statement()),
            If        => return Some(self.if_statement()),
            While     => return Some(self.while_statement()),
            Semicolon => return self.statement()
        };

        if self.lookahead().is_some() {
            Some(self.expression_statement())
        } else {
            None
        }
    }
}

pub fn parse(source: String) -> Program {
    let mut parser = Parser::new(&source);
    let mut program = Program { body: Vec::new() };

    while let Some(statement) = parser.statement() {
        program.body.push(statement);
    }

    return program;
}
