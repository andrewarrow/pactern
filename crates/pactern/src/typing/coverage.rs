//! Pattern-matrix usefulness: specializes finite constructors and uses a default
//! matrix for infinite domains. Payload columns preserve correlations, unlike
//! checking each payload independently. Work is bounded for adversarial input.

use super::*;

const MAX_COVERAGE_STEPS: usize = 20_000;
const MAX_COVERAGE_DEPTH: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Constructor {
    Bool(bool),
    Variant(SymbolId),
    Integer(String),
    String(String),
}

#[derive(Clone, Debug)]
pub(super) enum Pattern {
    Wild,
    Construct(Constructor, Vec<Pattern>),
}

type Matrix = Vec<Vec<Pattern>>;

impl Checker<'_> {
    pub(super) fn check_coverage(
        &mut self,
        file: FileId,
        span: Span,
        input: &Ty,
        patterns: Vec<(Span, Pattern)>,
    ) {
        let mut matrix = vec![];
        let mut steps = MAX_COVERAGE_STEPS;
        for (arm_span, pattern) in patterns {
            match self.useful(
                &matrix,
                std::slice::from_ref(&pattern),
                std::slice::from_ref(input),
                &mut steps,
                0,
            ) {
                Ok(false) => self.error(
                    file,
                    arm_span,
                    "PTN_TYPE_UNREACHABLE_MATCH_ARM",
                    "match arm is already covered by earlier arms",
                    None,
                ),
                Ok(true) => {}
                Err(()) => {
                    self.coverage_limit(file, span);
                    return;
                }
            }
            matrix.push(vec![pattern]);
        }
        match self.useful(
            &matrix,
            &[Pattern::Wild],
            std::slice::from_ref(input),
            &mut steps,
            0,
        ) {
            Ok(true) => self.error(
                file,
                span,
                "PTN_TYPE_NONEXHAUSTIVE_MATCH",
                "match does not cover every value; add the missing patterns or a catch-all arm",
                None,
            ),
            Ok(false) => {}
            Err(()) => self.coverage_limit(file, span),
        }
    }

    fn coverage_limit(&mut self, file: FileId, span: Span) {
        self.error(
            file,
            span,
            "PTN_TYPE_MATCH_LIMIT",
            "match coverage analysis limit exceeded; simplify the patterns",
            None,
        );
    }

    fn finite_constructors(&self, ty: &Ty) -> Option<Vec<(Constructor, Vec<Ty>)>> {
        let builtin = |parent: &str, names: Vec<(&str, Vec<Ty>)>| {
            names
                .into_iter()
                .map(|(name, payload)| {
                    let id = self
                        .out
                        .resolution
                        .symbols
                        .iter()
                        .find(|s| {
                            s.kind == SymbolKind::BuiltinVariant
                                && s.name == name
                                && self.out.resolution.symbols[s.parent.unwrap().0].name == parent
                        })
                        .unwrap()
                        .id;
                    (Constructor::Variant(id), payload)
                })
                .collect()
        };
        match self.prune(ty) {
            Ty::Bool => Some(vec![
                (Constructor::Bool(false), vec![]),
                (Constructor::Bool(true), vec![]),
            ]),
            Ty::Option(t) => Some(builtin(
                "Option",
                vec![("None", vec![]), ("Some", vec![*t])],
            )),
            Ty::Result(t, e) => Some(builtin("Result", vec![("Ok", vec![*t]), ("Err", vec![*e])])),
            Ty::Named(id) | Ty::Applied(id, _) => self.variants.get(&id).map(|vs| {
                vs.iter()
                    .map(|(id, payload)| {
                        (
                            Constructor::Variant(*id),
                            payload
                                .iter()
                                .map(|t| Self::substitute(t, &self.type_substitutions(ty)))
                                .collect(),
                        )
                    })
                    .collect()
            }),
            _ => None,
        }
    }

    fn useful(
        &self,
        matrix: &Matrix,
        row: &[Pattern],
        types: &[Ty],
        steps: &mut usize,
        depth: usize,
    ) -> Result<bool, ()> {
        let cost = 1 + matrix.len() + row.len();
        if *steps < cost || depth > MAX_COVERAGE_DEPTH {
            return Err(());
        }
        *steps -= cost;
        if types.iter().any(|ty| !self.bounded(ty)) {
            return Err(());
        }
        if row.is_empty() {
            return Ok(matrix.is_empty());
        }
        // A catch-all row covers the entire remaining product without expansion.
        if matrix
            .iter()
            .any(|r| r.iter().all(|p| matches!(p, Pattern::Wild)))
        {
            return Ok(false);
        }
        match &row[0] {
            Pattern::Construct(constructor, payload) => {
                let payload_types = self
                    .finite_constructors(&types[0])
                    .and_then(|cs| cs.into_iter().find(|(c, _)| c == constructor))
                    .map(|(_, ts)| ts)
                    .unwrap_or_default();
                let specialized = specialize(matrix, constructor, payload.len());
                let next_row: Vec<_> = payload
                    .iter()
                    .cloned()
                    .chain(row[1..].iter().cloned())
                    .collect();
                let next_types: Vec<_> = payload_types
                    .into_iter()
                    .chain(types[1..].iter().cloned())
                    .collect();
                self.useful(&specialized, &next_row, &next_types, steps, depth + 1)
            }
            Pattern::Wild => {
                let present: BTreeSet<_> = matrix
                    .iter()
                    .filter_map(|r| match &r[0] {
                        Pattern::Construct(c, _) => Some(c),
                        Pattern::Wild => None,
                    })
                    .collect();
                if let Some(constructors) = self.finite_constructors(&types[0])
                    && constructors.iter().all(|(c, _)| present.contains(c))
                {
                    for (constructor, payload) in constructors {
                        let specialized = specialize(matrix, &constructor, payload.len());
                        let next_row: Vec<_> = std::iter::repeat_n(Pattern::Wild, payload.len())
                            .chain(row[1..].iter().cloned())
                            .collect();
                        let next_types: Vec<_> = payload
                            .into_iter()
                            .chain(types[1..].iter().cloned())
                            .collect();
                        if self.useful(&specialized, &next_row, &next_types, steps, depth + 1)? {
                            return Ok(true);
                        }
                    }
                    return Ok(false);
                }
                let default: Matrix = matrix
                    .iter()
                    .filter(|r| matches!(r[0], Pattern::Wild))
                    .map(|r| r[1..].to_vec())
                    .collect();
                self.useful(&default, &row[1..], &types[1..], steps, depth + 1)
            }
        }
    }
}

fn specialize(matrix: &Matrix, constructor: &Constructor, arity: usize) -> Matrix {
    matrix
        .iter()
        .filter_map(|row| {
            let mut specialized = match &row[0] {
                Pattern::Wild => vec![Pattern::Wild; arity],
                Pattern::Construct(c, payload) if c == constructor => payload.clone(),
                _ => return None,
            };
            specialized.extend_from_slice(&row[1..]);
            Some(specialized)
        })
        .collect()
}
