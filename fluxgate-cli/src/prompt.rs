//! Interactive questions, replaceable in tests.

use std::collections::VecDeque;
use std::io::{BufRead, Write};

use crate::error::CliError;

pub trait Prompter {
    /// One line of text; an empty answer gives `default`.
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError>;
    /// Hidden input such as a password or token.
    fn secret(&mut self, label: &str) -> Result<String, CliError>;
    /// Index of the chosen option.
    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError>;
}

/// Asks on stderr and reads stdin, so stdout stays clean for results.
pub struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut err = std::io::stderr();
        match default {
            Some(default) => write!(err, "{label} [{default}]: ")?,
            None => write!(err, "{label}: ")?,
        }
        err.flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(CliError::Usage(format!("no input for '{label}'")));
        }
        let line = line.trim();
        Ok(if line.is_empty() { default.unwrap_or_default().to_string() } else { line.to_string() })
    }

    fn secret(&mut self, label: &str) -> Result<String, CliError> {
        rpassword::prompt_password(format!("{label}: "))
            .map_err(|err| CliError::Usage(format!("cannot read '{label}': {err}")))
    }

    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError> {
        if options.is_empty() {
            return Err(CliError::Usage(format!("nothing to choose for '{label}'")));
        }
        let mut err = std::io::stderr();
        writeln!(err, "{label}:")?;
        for (index, option) in options.iter().enumerate() {
            writeln!(err, "  {}) {option}", index + 1)?;
        }
        loop {
            let answer = self.input("Choose a number", Some("1"))?;
            match answer.parse::<usize>() {
                Ok(number) if (1..=options.len()).contains(&number) => return Ok(number - 1),
                _ => writeln!(err, "enter a number from 1 to {}", options.len())?,
            }
        }
    }
}

/// Answers questions from a list, for tests.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    answers: VecDeque<String>,
    /// Labels of the questions asked, in order.
    pub asked: Vec<String>,
}

impl ScriptedPrompter {
    pub fn new(answers: &[&str]) -> Self {
        Self { answers: answers.iter().map(|a| a.to_string()).collect(), asked: Vec::new() }
    }

    fn next(&mut self, label: &str) -> Result<String, CliError> {
        self.asked.push(label.to_string());
        self.answers
            .pop_front()
            .ok_or_else(|| CliError::Usage(format!("no scripted answer for '{label}'")))
    }
}

impl Prompter for ScriptedPrompter {
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let answer = self.next(label)?;
        Ok(if answer.is_empty() { default.unwrap_or_default().to_string() } else { answer })
    }

    fn secret(&mut self, label: &str) -> Result<String, CliError> {
        self.next(label)
    }

    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError> {
        let answer = self.next(label)?;
        options
            .iter()
            .position(|option| option == &answer)
            .or_else(|| answer.parse::<usize>().ok().filter(|index| *index < options.len()))
            .ok_or_else(|| {
                CliError::Usage(format!("scripted answer '{answer}' is not an option for '{label}'"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_answers_in_order_with_defaults_and_selection_by_text() {
        let options = vec!["table".to_string(), "json".to_string()];
        let mut prompter = ScriptedPrompter::new(&["", "alice", "pw", "json", "0"]);
        assert_eq!(prompter.input("URL", Some("http://h")).unwrap(), "http://h");
        assert_eq!(prompter.input("Username", None).unwrap(), "alice");
        assert_eq!(prompter.secret("Password").unwrap(), "pw");
        assert_eq!(prompter.select("Output", &options).unwrap(), 1);
        assert_eq!(prompter.select("Output", &options).unwrap(), 0);
        assert_eq!(prompter.asked, vec!["URL", "Username", "Password", "Output", "Output"]);
        assert!(prompter.input("More", None).is_err());
    }

    #[test]
    fn scripted_select_rejects_unknown_answers() {
        let mut prompter = ScriptedPrompter::new(&["yaml"]);
        assert!(prompter.select("Output", &["json".to_string()]).is_err());
    }
}
