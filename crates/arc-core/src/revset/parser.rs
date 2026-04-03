use anyhow::{Result, anyhow, bail};
use pest::iterators::Pair;

mod generated {
    #![allow(missing_docs)]

    use pest_derive::Parser;

    #[derive(Parser)]
    #[grammar = "revset/revset.pest"]
    pub(super) struct RevsetParser;
}

use generated::{RevsetParser, Rule};
use pest::Parser;

/// AST for a parsed revset expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevsetExpression {
    /// A single symbol such as a view name, hash prefix, or `@`.
    Symbol(String),
    /// A function call with zero or more expression arguments.
    Function {
        /// Function name.
        name: String,
        /// Function arguments.
        args: Vec<RevsetExpression>,
    },
    /// Set intersection (`&`).
    Intersection(Box<RevsetExpression>, Box<RevsetExpression>),
    /// Set union (`|`).
    Union(Box<RevsetExpression>, Box<RevsetExpression>),
}

/// Parse user input into a revset AST.
pub fn parse(input: &str) -> Result<RevsetExpression> {
    let mut pairs = RevsetParser::parse(Rule::query, input)?;
    let query = pairs.next().ok_or_else(|| anyhow!("expected query"))?;
    let expression = query
        .into_inner()
        .next()
        .ok_or_else(|| anyhow!("query has no expression"))?;
    build_expression(expression)
}

fn build_expression(pair: Pair<Rule>) -> Result<RevsetExpression> {
    match pair.as_rule() {
        Rule::expression => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| anyhow!("expression has no inner node"))?;
            build_expression(inner)
        }
        Rule::union => build_union(pair),
        Rule::intersection => build_intersection(pair),
        Rule::primary => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| anyhow!("primary has no inner node"))?;
            build_expression(inner)
        }
        Rule::function_call => build_function(pair),
        Rule::symbol => Ok(RevsetExpression::Symbol(pair.as_str().to_string())),
        _ => bail!("unexpected parse rule: {:?}", pair.as_rule()),
    }
}

fn build_union(pair: Pair<Rule>) -> Result<RevsetExpression> {
    let mut parts = pair.into_inner().map(build_expression);
    let first = parts
        .next()
        .ok_or_else(|| anyhow!("union has no operands"))??;
    parts.try_fold(first, |acc, next| {
        Ok(RevsetExpression::Union(Box::new(acc), Box::new(next?)))
    })
}

fn build_intersection(pair: Pair<Rule>) -> Result<RevsetExpression> {
    let mut parts = pair.into_inner().map(build_expression);
    let first = parts
        .next()
        .ok_or_else(|| anyhow!("intersection has no operands"))??;
    parts.try_fold(first, |acc, next| {
        Ok(RevsetExpression::Intersection(
            Box::new(acc),
            Box::new(next?),
        ))
    })
}

fn build_function(pair: Pair<Rule>) -> Result<RevsetExpression> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or_else(|| anyhow!("function call missing name"))?
        .as_str()
        .to_string();

    let args = match inner.next() {
        Some(args_pair) if args_pair.as_rule() == Rule::args => args_pair
            .into_inner()
            .map(build_expression)
            .collect::<Result<Vec<_>>>()?,
        Some(other) => {
            return Err(anyhow!(
                "unexpected function inner rule: {:?}",
                other.as_rule()
            ));
        }
        None => Vec::new(),
    };

    Ok(RevsetExpression::Function { name, args })
}

#[cfg(test)]
mod tests {
    use super::{RevsetExpression, parse};

    #[test]
    fn parses_function_intersection_with_current_symbol() {
        let parsed = parse("ancestors(main) & @").expect("revset should parse");

        let expected = RevsetExpression::Intersection(
            Box::new(RevsetExpression::Function {
                name: "ancestors".to_string(),
                args: vec![RevsetExpression::Symbol("main".to_string())],
            }),
            Box::new(RevsetExpression::Symbol("@".to_string())),
        );

        assert_eq!(parsed, expected);
    }
}
