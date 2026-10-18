//! Bounded pure execution of checked core IR. Pacts are not executed.
use crate::{
    core::*,
    resolution::{Location, SymbolId},
    syntax::{BinaryOp, UnaryOp},
    typing::ValueType,
};
use std::collections::BTreeMap;

/// Immutable language values. Symbol IDs belong to the supplied program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    I64(i64),
    String(String),
    Unit,
    List(Vec<Value>),
    Record(BTreeMap<SymbolId, Value>),
    Variant {
        variant: SymbolId,
        payload: Vec<Value>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrapKind {
    Overflow,
    DivisionByZero,
    IndexOutOfBounds,
    ExecutionLimit,
    InvalidInvocation,
    Invariant,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trap {
    pub kind: TrapKind,
    pub origin: Option<Location>,
}
impl std::fmt::Display for Trap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} at {:?}", self.kind, self.origin)
    }
}
impl std::error::Error for Trap {}
/// Fuel counts statements, expressions, function entries, and produced/copied
/// value nodes (plus UTF-8 bytes). Value nesting is capped at 64. Call depth is capped at
/// 64 even when a caller requests more, to protect the Rust host stack.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub fuel: u64,
    pub call_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: 100_000,
            call_depth: 64,
        }
    }
}
type Result<T> = std::result::Result<T, Trap>;
type Env = BTreeMap<SymbolId, Value>;
enum Flow {
    Next,
    Return(Value),
    Break,
    Continue,
}

/// Invoke a nongeneric entry point. Generic helpers execute with erased types.
/// Invalid IDs, arities, and argument types are rejected before execution.
/// No filesystem, host effects, or contract checks are performed.
pub fn execute(
    program: &Program,
    function: SymbolId,
    arguments: Vec<Value>,
    limits: Limits,
) -> Result<Value> {
    let mut fuel = limits.fuel;
    for value in &arguments {
        fuel -= value_cost(value, fuel).ok_or(Trap {
            kind: TrapKind::ExecutionLimit,
            origin: None,
        })?;
    }
    let invalid = || Trap {
        kind: TrapKind::InvalidInvocation,
        origin: None,
    };
    let d = program
        .declarations()
        .iter()
        .find(|d| d.symbol == function)
        .ok_or_else(invalid)?;
    let DeclarationKind::Function {
        type_parameters,
        parameters,
        ..
    } = &d.kind
    else {
        return Err(invalid());
    };
    if !type_parameters.is_empty()
        || arguments.len() != parameters.len()
        || !parameters.iter().zip(&arguments).all(|(id, v)| {
            program
                .symbol_types()
                .get(id)
                .is_some_and(|t| accepts(program, t, v, 0))
        })
    {
        return Err(invalid());
    }
    Machine {
        program,
        fuel,
        max_depth: limits.call_depth.min(64),
        depth: 0,
    }
    .call(function, arguments, d.origin)
}
fn accepts(p: &Program, t: &ValueType, v: &Value, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    match (t, v) {
        (ValueType::Bool, Value::Bool(_))
        | (ValueType::I64, Value::I64(_))
        | (ValueType::String, Value::String(_))
        | (ValueType::Unit, Value::Unit) => true,
        (ValueType::List(t), Value::List(vs)) => vs.iter().all(|v| accepts(p, t, v, depth + 1)),
        (ValueType::Option(_), Value::Variant { variant, payload })
        | (ValueType::Result(_, _), Value::Variant { variant, payload }) => {
            let Some(s) = p.resolution().symbols.get(variant.0) else {
                return false;
            };
            if s.kind != crate::resolution::SymbolKind::BuiltinVariant {
                return false;
            }
            match (t, s.name.as_str(), payload.as_slice()) {
                (ValueType::Option(_), "None", []) => true,
                (ValueType::Option(inner), "Some", [v])
                | (ValueType::Result(inner, _), "Ok", [v])
                | (ValueType::Result(_, inner), "Err", [v]) => accepts(p, inner, v, depth + 1),
                _ => false,
            }
        }
        (ValueType::Named(id), _) | (ValueType::Applied(id, _), _) => {
            let Some(d) = p.declarations().iter().find(|d| d.symbol == *id) else {
                return false;
            };
            let args = if let ValueType::Applied(_, args) = t {
                args.as_slice()
            } else {
                &[]
            };
            match (&d.kind, v) {
                (
                    DeclarationKind::Struct {
                        type_parameters,
                        fields,
                    },
                    Value::Record(values),
                ) => {
                    let subs = type_parameters
                        .iter()
                        .copied()
                        .zip(args.iter().cloned())
                        .collect();
                    fields.len() == values.len()
                        && fields.iter().all(|f| {
                            values
                                .get(f)
                                .zip(p.symbol_types().get(f))
                                .is_some_and(|(v, t)| {
                                    accepts(p, &crate::core::substitute(t, &subs), v, depth + 1)
                                })
                        })
                }
                (
                    DeclarationKind::Enum {
                        type_parameters,
                        variants,
                    },
                    Value::Variant { variant, payload },
                ) => {
                    let subs = type_parameters
                        .iter()
                        .copied()
                        .zip(args.iter().cloned())
                        .collect();
                    variants
                        .iter()
                        .find(|v| v.symbol == *variant)
                        .is_some_and(|v| {
                            v.payload.len() == payload.len()
                                && v.payload.iter().zip(payload).all(|(t, v)| {
                                    accepts(p, &crate::core::substitute(t, &subs), v, depth + 1)
                                })
                        })
                }
                _ => false,
            }
        }
        _ => false,
    }
}
struct Machine<'a> {
    program: &'a Program,
    fuel: u64,
    max_depth: usize,
    depth: usize,
}
impl Machine<'_> {
    fn trap(&self, kind: TrapKind, origin: Location) -> Trap {
        Trap {
            kind,
            origin: Some(origin),
        }
    }
    fn tick(&mut self, origin: Location) -> Result<()> {
        if self.fuel == 0 {
            return Err(self.trap(TrapKind::ExecutionLimit, origin));
        }
        self.fuel -= 1;
        Ok(())
    }
    fn call(&mut self, id: SymbolId, args: Vec<Value>, origin: Location) -> Result<Value> {
        self.tick(origin)?;
        if self.depth >= self.max_depth {
            return Err(self.trap(TrapKind::ExecutionLimit, origin));
        }
        let d = self
            .program
            .declarations()
            .iter()
            .find(|d| d.symbol == id)
            .ok_or_else(|| self.trap(TrapKind::Invariant, origin))?;
        let DeclarationKind::Function {
            parameters, body, ..
        } = &d.kind
        else {
            return Err(self.trap(TrapKind::Invariant, origin));
        };
        let mut env = parameters.iter().copied().zip(args).collect();
        self.depth += 1;
        let result = self.block(body, &mut env);
        self.depth -= 1;
        match result? {
            Flow::Return(v) => Ok(v),
            Flow::Next => Ok(Value::Unit),
            _ => Err(self.trap(TrapKind::Invariant, origin)),
        }
    }
    fn block(&mut self, b: &Block, env: &mut Env) -> Result<Flow> {
        for s in &b.statements {
            let flow = self.statement(s, env)?;
            if !matches!(flow, Flow::Next) {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }
    fn statement(&mut self, s: &Statement, env: &mut Env) -> Result<Flow> {
        self.tick(s.origin)?;
        match &s.kind {
            StatementKind::Let { symbol, value, .. } | StatementKind::Assign { symbol, value } => {
                let v = self.expr(value, env)?;
                env.insert(*symbol, v);
            }
            StatementKind::Expression(e) => {
                self.expr(e, env)?;
            }
            StatementKind::Return(e) => {
                return Ok(Flow::Return(match e {
                    Some(e) => self.expr(e, env)?,
                    None => Value::Unit,
                }));
            }
            StatementKind::Break => return Ok(Flow::Break),
            StatementKind::Continue => return Ok(Flow::Continue),
            StatementKind::Block(b) => return self.block(b, env),
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if self.expr(condition, env)? == Value::Bool(true) {
                    return self.block(then_branch, env);
                }
                if let Some(s) = else_branch {
                    return self.statement(s, env);
                }
            }
            StatementKind::While { condition, body } => {
                while self.expr(condition, env)? == Value::Bool(true) {
                    match self.block(body, env)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
        }
        Ok(Flow::Next)
    }
    fn expr(&mut self, e: &Expression, env: &mut Env) -> Result<Value> {
        self.tick(e.origin)?;
        let bad = || Trap {
            kind: TrapKind::Invariant,
            origin: Some(e.origin),
        };
        let value = match &e.kind {
            ExpressionKind::Bool(v) => Value::Bool(*v),
            ExpressionKind::I64(v) => Value::I64(*v),
            ExpressionKind::String(v) => Value::String(v.clone()),
            ExpressionKind::Unit => Value::Unit,
            ExpressionKind::Read(id) => env.get(id).cloned().ok_or_else(bad)?,
            ExpressionKind::List(es) => Value::List(
                es.iter()
                    .map(|e| self.expr(e, env))
                    .collect::<Result<_>>()?,
            ),
            ExpressionKind::Record { fields } => {
                let mut values = BTreeMap::new();
                for (id, e) in fields {
                    values.insert(*id, self.expr(e, env)?);
                }
                Value::Record(values)
            }
            ExpressionKind::Construct { variant, arguments } => Value::Variant {
                variant: *variant,
                payload: arguments
                    .iter()
                    .map(|e| self.expr(e, env))
                    .collect::<Result<_>>()?,
            },
            ExpressionKind::Call {
                function,
                arguments,
                ..
            } => {
                let args = arguments
                    .iter()
                    .map(|e| self.expr(e, env))
                    .collect::<Result<_>>()?;
                self.call(*function, args, e.origin)?
            }
            ExpressionKind::Field { base, field } => {
                let Value::Record(mut values) = self.expr(base, env)? else {
                    return Err(bad());
                };
                values.remove(field).ok_or_else(bad)?
            }
            ExpressionKind::Index { base, index } => {
                let b = self.expr(base, env)?;
                let i = self.expr(index, env)?;
                let (Value::List(v), Value::I64(i)) = (b, i) else {
                    return Err(bad());
                };
                usize::try_from(i)
                    .ok()
                    .and_then(|i| v.get(i))
                    .cloned()
                    .ok_or_else(|| self.trap(TrapKind::IndexOutOfBounds, e.origin))?
            }
            ExpressionKind::Unary { operator, operand } => {
                match (operator, self.expr(operand, env)?) {
                    (UnaryOp::Not, Value::Bool(v)) => Value::Bool(!v),
                    (UnaryOp::Negate, Value::I64(v)) => Value::I64(
                        v.checked_neg()
                            .ok_or_else(|| self.trap(TrapKind::Overflow, e.origin))?,
                    ),
                    _ => return Err(bad()),
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let l = self.expr(left, env)?;
                if (*operator == BinaryOp::And && l == Value::Bool(false))
                    || (*operator == BinaryOp::Or && l == Value::Bool(true))
                {
                    l
                } else {
                    let r = self.expr(right, env)?;
                    self.binary(l, *operator, r, e.origin)?
                }
            }
            ExpressionKind::Match { scrutinee, arms } => {
                let v = self.expr(scrutinee, env)?;
                let mut found = None;
                for a in arms {
                    let mut bindings = BTreeMap::new();
                    if matches_pattern(&a.pattern, &v, &mut bindings) {
                        // Pattern bindings copy values just like ordinary reads.
                        // Charge before cloning, and only clone a selected arm's
                        // bindings after the complete pattern has matched.
                        for (id, value) in &bindings {
                            let cost = value_cost(value, self.fuel).ok_or_else(|| {
                                self.trap(TrapKind::ExecutionLimit, a.pattern.origin)
                            })?;
                            self.fuel -= cost;
                            env.insert(*id, (*value).clone());
                        }
                        let result = self.expr(&a.value, env);
                        for id in bindings.keys() {
                            env.remove(id);
                        }
                        found = Some(result?);
                        break;
                    }
                }
                found.ok_or_else(bad)?
            }
        };
        let cost = value_cost(&value, self.fuel)
            .ok_or_else(|| self.trap(TrapKind::ExecutionLimit, e.origin))?;
        self.fuel -= cost;
        Ok(value)
    }
    fn binary(&self, l: Value, op: BinaryOp, r: Value, origin: Location) -> Result<Value> {
        use BinaryOp::*;
        if op == Equal {
            return Ok(Value::Bool(l == r));
        }
        if op == NotEqual {
            return Ok(Value::Bool(l != r));
        }
        if let (Value::Bool(a), Value::Bool(b)) = (&l, &r) {
            return match op {
                And => Ok(Value::Bool(*a && *b)),
                Or => Ok(Value::Bool(*a || *b)),
                _ => Err(self.trap(TrapKind::Invariant, origin)),
            };
        }
        let (Value::I64(a), Value::I64(b)) = (l, r) else {
            return Err(self.trap(TrapKind::Invariant, origin));
        };
        match op {
            Less => return Ok(Value::Bool(a < b)),
            LessEqual => return Ok(Value::Bool(a <= b)),
            Greater => return Ok(Value::Bool(a > b)),
            GreaterEqual => return Ok(Value::Bool(a >= b)),
            Divide | Remainder if b == 0 => return Err(self.trap(TrapKind::DivisionByZero, origin)),
            _ => {}
        }
        let v = match op {
            Add => a.checked_add(b),
            Subtract => a.checked_sub(b),
            Multiply => a.checked_mul(b),
            Divide => a.checked_div(b),
            Remainder => a.checked_rem(b),
            _ => return Err(self.trap(TrapKind::Invariant, origin)),
        };
        v.map(Value::I64)
            .ok_or_else(|| self.trap(TrapKind::Overflow, origin))
    }
}
fn matches_pattern<'a>(p: &Pattern, v: &'a Value, env: &mut BTreeMap<SymbolId, &'a Value>) -> bool {
    match (&p.kind, v) {
        (PatternKind::Wildcard, _) => true,
        (PatternKind::Binding(id), _) => {
            env.insert(*id, v);
            true
        }
        (PatternKind::Bool(a), Value::Bool(b)) => a == b,
        (PatternKind::I64(a), Value::I64(b)) => a == b,
        (PatternKind::String(a), Value::String(b)) => a == b,
        (
            PatternKind::Variant {
                variant: a,
                payload: ps,
            },
            Value::Variant {
                variant: b,
                payload: vs,
            },
        ) => {
            a == b
                && ps.len() == vs.len()
                && ps.iter().zip(vs).all(|(p, v)| matches_pattern(p, v, env))
        }
        _ => false,
    }
}

// Charge immutable value copies and construction as well as evaluation. Bound
// nesting independently so cloning, matching and destruction stay stack-safe.
fn value_cost(value: &Value, budget: u64) -> Option<u64> {
    fn visit(v: &Value, depth: usize, remaining: &mut u64) -> Option<()> {
        if depth > 64 {
            return None;
        }
        let cost = match v {
            Value::String(s) => 1u64.checked_add(s.len() as u64)?,
            _ => 1,
        };
        *remaining = remaining.checked_sub(cost)?;
        match v {
            Value::List(vs) | Value::Variant { payload: vs, .. } => {
                for v in vs {
                    visit(v, depth + 1, remaining)?;
                }
            }
            Value::Record(fields) => {
                for v in fields.values() {
                    visit(v, depth + 1, remaining)?;
                }
            }
            _ => {}
        }
        Some(())
    }
    let mut remaining = budget;
    visit(value, 0, &mut remaining)?;
    Some(budget - remaining)
}
