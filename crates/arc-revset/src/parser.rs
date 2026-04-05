use anyhow::{Result, anyhow, bail};
use pest::iterators::Pair;

mod generated {
    #![allow(missing_docs)]

    use pest_derive::Parser;

    #[derive(Parser)]
    #[grammar = "revset.pest"]
    pub(super) struct RevsetParser;
}

use generated::{RevsetParser, Rule};
use pest::Parser;

/// AST for a parsed revset expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevsetExpression {
    /// A single symbol such as a view name, hash prefix, or `@`.
    Symbol(String),
    /// A quoted string literal argument.
    StringLiteral(String),
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
        Rule::string_literal => Ok(RevsetExpression::StringLiteral(unescape_string_literal(
            pair.as_str(),
        )?)),
        Rule::symbol => Ok(RevsetExpression::Symbol(pair.as_str().to_string())),
        _ => bail!("unexpected parse rule: {:?}", pair.as_rule()),
    }
}

fn unescape_string_literal(raw: &str) -> Result<String> {
    if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
        bail!("invalid string literal: {raw}");
    }

    let mut out = String::with_capacity(raw.len().saturating_sub(2));
    let mut chars = raw[1..raw.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars
                .next()
                .ok_or_else(|| anyhow!("unterminated escape in string literal"))?;
            match escaped {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                other => bail!("unsupported escape sequence: \\{other}"),
            }
        } else {
            out.push(ch);
        }
    }

    Ok(out)
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

    #[test]
    fn parses_touched_with_string_path_argument() {
        let parsed = parse("touched(\"src/main.rs\")").expect("revset should parse");

        let expected = RevsetExpression::Function {
            name: "touched".to_string(),
            args: vec![RevsetExpression::StringLiteral("src/main.rs".to_string())],
        };

        assert_eq!(parsed, expected);
    }

    #[test]
    fn rejects_unterminated_string_literal() {
        let err = parse("touched(\"src/main.rs)").expect_err("unterminated literal must fail");
        assert!(!err.to_string().is_empty());
    }
}
