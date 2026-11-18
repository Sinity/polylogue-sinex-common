//! Classify a selection and split declared steps without reading process state.
//! Products supply selection predicates and consume their local options;
//! typed argument parsing remains with `StepDeclarations::parse_step`.
use crate::{ArgumentForm, Failure, StepDeclarations};
use serde_json::Value;
use std::convert::Infallible;

/// Shared precedence categories. Extra product selection forms come after
/// units and row numbers and before references, in the order the product defines.
#[derive(Debug, PartialEq, Eq)]
pub enum FirstWord<Extra = Infallible> {
    Nothing,
    Stdin,
    Command,
    Verb,
    SavedQuery { name: String },
    Unit,
    RowNumbers,
    Reference,
    Additional(Extra),
    QueryText,
    SearchWord,
}

/// Predicates operate on already-loaded product catalogs. The parser neither
/// opens an archive nor decides whether a reference is currently available.
pub trait SelectionRules {
    type Additional;
    fn is_command(&self, word: &str) -> bool;
    fn saved_name(&self, word: &str) -> Option<String>;
    fn is_unit(&self, word: &str) -> bool;
    fn is_row_numbers(&self, word: &str) -> bool;
    fn is_reference(&self, word: &str) -> bool;
    fn additional(&self, word: &str) -> Option<Self::Additional>;
    fn is_query(&self, word: &str) -> bool;
}

pub fn classify_first<R: SelectionRules>(
    word: Option<&str>,
    rules: &R,
    declarations: &StepDeclarations,
) -> FirstWord<R::Additional> {
    let Some(word) = word else {
        return FirstWord::Nothing;
    };
    if word == "-" {
        return FirstWord::Stdin;
    }
    if rules.is_command(word) {
        return FirstWord::Command;
    }
    if declarations.verbs().iter().any(|verb| verb.verb() == word) {
        return FirstWord::Verb;
    }
    if let Some(name) = rules.saved_name(word) {
        return FirstWord::SavedQuery { name };
    }
    if rules.is_unit(word) {
        return FirstWord::Unit;
    }
    if rules.is_row_numbers(word) {
        return FirstWord::RowNumbers;
    }
    if let Some(additional) = rules.additional(word) {
        return FirstWord::Additional(additional);
    }
    if rules.is_reference(word) {
        return FirstWord::Reference;
    }
    if rules.is_query(word) {
        FirstWord::QueryText
    } else {
        FirstWord::SearchWord
    }
}

/// A word consumed by a declared field. Products can locate stdin arguments
/// without inferring their meaning from a rendered or re-encoded request.
#[derive(Debug, PartialEq, Eq)]
pub struct ArgumentWord {
    pub field: String,
    pub index: usize,
    pub positional: bool,
}

#[derive(Debug)]
pub struct SplitStep {
    pub end: usize,
    pub step: Value,
    pub argument_words: Vec<ArgumentWord>,
}

/// Structural errors retain word positions for the product's correction text.
#[derive(Debug)]
pub enum SplitError<Local> {
    UnknownVerb { index: usize },
    UnknownOption { index: usize },
    MissingValue { index: usize },
    MissingArgument { index: usize, name: String },
    Arguments(Failure),
    Local(Local),
}

impl StepDeclarations {
    /// Split and type-check one step. `local` may consume a product option by
    /// advancing its index to the last consumed word and returning true.
    /// A required positional consumes a verb-looking word; an optional one
    /// leaves it for the next step. `--` makes the following positional literal.
    pub fn split_step<E>(
        &self,
        args: &[String],
        start: usize,
        mut local: impl FnMut(&str, &[String], &mut usize) -> Result<bool, E>,
    ) -> Result<SplitStep, SplitError<E>> {
        let declaration = args
            .get(start)
            .and_then(|word| self.verbs().iter().find(|verb| verb.verb() == word))
            .ok_or(SplitError::UnknownVerb { index: start })?;
        let positionals: Vec<_> = crate::arguments::projection(&declaration.description, &[])
            .into_iter()
            .filter(|field| field.spec.form == ArgumentForm::Positional)
            .collect();
        let mut positional = 0;
        let mut index = start + 1;
        let mut words = Vec::new();
        let mut argument_words = Vec::new();
        while index < args.len() {
            if local(declaration.verb(), args, &mut index).map_err(SplitError::Local)? {
                index += 1;
                continue;
            }
            if args[index] == "--" && positional < positionals.len() {
                words.push("--".into());
                index += 1;
                let value = args
                    .get(index)
                    .ok_or(SplitError::MissingValue { index: index - 1 })?;
                argument_words.push(ArgumentWord {
                    field: positionals[positional].spec.name.clone(),
                    index,
                    positional: true,
                });
                words.push(value.clone());
                positional += 1;
                index += 1;
                continue;
            }
            let word = &args[index];
            if word.starts_with('-') && word != "-" {
                let name = word.trim_start_matches('-').replace('-', "_");
                let field = declaration
                    .arguments()
                    .iter()
                    .find(|field| {
                        field.name == name && field.form != ArgumentForm::Positional && field.listed
                    })
                    .ok_or(SplitError::UnknownOption { index })?;
                words.push(word.clone());
                if field.form != ArgumentForm::Flag {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or(SplitError::MissingValue { index: index - 1 })?;
                    argument_words.push(ArgumentWord {
                        field: field.name.clone(),
                        index,
                        positional: false,
                    });
                    words.push(value.clone());
                }
                index += 1;
                continue;
            }
            let next_verb = self.verbs().iter().any(|verb| verb.verb() == word);
            let Some(field) = positionals
                .get(positional)
                .filter(|field| field.required || !next_verb)
            else {
                break;
            };
            argument_words.push(ArgumentWord {
                field: field.spec.name.clone(),
                index,
                positional: true,
            });
            words.push(word.clone());
            positional += 1;
            index += 1;
        }
        if let Some(field) = positionals[positional..]
            .iter()
            .find(|field| field.required)
        {
            return Err(SplitError::MissingArgument {
                index,
                name: field.spec.name.clone(),
            });
        }
        let step = self
            .parse_step(declaration.verb(), &words)
            .map_err(SplitError::Arguments)?;
        Ok(SplitStep {
            end: index,
            step,
            argument_words,
        })
    }
}

#[cfg(test)]
mod tests;
