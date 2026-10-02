//! Wire format of `POST /v1/systemone` (https://docs.typesafe.ai/api.md).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MIN_CHOICE_OPTIONS: usize = 2;
pub const MAX_CHOICE_OPTIONS: usize = 255;
pub const MIN_SCORE_LEVELS: usize = 2;
pub const MAX_SCORE_LEVELS: usize = 10;

/// What a yes and a no mean for a Noul question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub when_true: Value,
    #[serde(rename = "false")]
    pub when_false: Value,
}

/// One typed question. Build it with the constructors so limits are checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Option<Value>>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    #[error("a choice needs {MIN_CHOICE_OPTIONS} to {MAX_CHOICE_OPTIONS} options, got {0}")]
    ChoiceOptions(usize),
    #[error("a score needs {MIN_SCORE_LEVELS} to {MAX_SCORE_LEVELS} levels, got {0}")]
    ScoreLevels(usize),
}

impl Question {
    /// A yes/no question without criteria.
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    /// A yes/no question with descriptions of what yes and no mean.
    pub fn noul_with(
        instructions: impl Into<Value>,
        when_true: impl Into<Value>,
        when_false: impl Into<Value>,
    ) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: Some(NoulCriteria {
                when_true: when_true.into(),
                when_false: when_false.into(),
            }),
        }
    }

    /// Picks one option. `None` means the option needs no description.
    pub fn choice<I, K>(instructions: impl Into<Value>, options: I) -> Result<Self, QuestionError>
    where
        I: IntoIterator<Item = (K, Option<Value>)>,
        K: Into<String>,
    {
        let criteria: BTreeMap<String, Option<Value>> = options
            .into_iter()
            .map(|(key, description)| (key.into(), description))
            .collect();
        if !(MIN_CHOICE_OPTIONS..=MAX_CHOICE_OPTIONS).contains(&criteria.len()) {
            return Err(QuestionError::ChoiceOptions(criteria.len()));
        }
        Ok(Question::Choice {
            instructions: instructions.into(),
            criteria,
        })
    }

    /// Rates along ordered levels, lowest first.
    pub fn score<I, L>(instructions: impl Into<Value>, levels: I) -> Result<Self, QuestionError>
    where
        I: IntoIterator<Item = L>,
        L: Into<Value>,
    {
        let criteria: Vec<Value> = levels.into_iter().map(Into::into).collect();
        if !(MIN_SCORE_LEVELS..=MAX_SCORE_LEVELS).contains(&criteria.len()) {
            return Err(QuestionError::ScoreLevels(criteria.len()));
        }
        Ok(Question::Score {
            instructions: instructions.into(),
            criteria,
        })
    }
}

/// Request body. The model comes from config, so callers build `RequestParts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemOneRequest {
    pub state: Value,
    pub model: String,
    pub questions: BTreeMap<String, Question>,
}

/// State plus questions, built by a judgment handler.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestParts {
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    pub choice: String,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    pub score: f64,
    #[serde(default)]
    pub legend: BTreeMap<String, Value>,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul { noul: f64 },
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

/// Answers keyed by question id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Answers(pub BTreeMap<String, Answer>);

impl Answers {
    pub fn noul(&self, id: &str) -> Option<f64> {
        match self.0.get(id)? {
            Answer::Noul { noul } => Some(*noul),
            _ => None,
        }
    }

    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        match self.0.get(id)? {
            Answer::Choice(answer) => Some(answer),
            _ => None,
        }
    }

    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        match self.0.get(id)? {
            Answer::Score(answer) => Some(answer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: Answers,
    #[serde(default)]
    pub usage: Usage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_each_question_type() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "is_urgent".to_string(),
            Question::noul_with("Does this convey urgency?", "Time-sensitive", "No urgency"),
        );
        questions.insert(
            "department".to_string(),
            Question::choice(
                "Which team should handle this?",
                [
                    ("billing", Some(json!("Payments, invoicing, refunds"))),
                    ("technical", None),
                ],
            )
            .unwrap(),
        );
        questions.insert(
            "frustration".to_string(),
            Question::score(
                "How frustrated is the customer?",
                ["Calm", "Frustrated", "Very angry"],
            )
            .unwrap(),
        );
        questions.insert("plain".to_string(), Question::noul("Is it plain?"));
        let request = SystemOneRequest {
            state: json!("Help! My payouts have been failing for 3 days."),
            model: "jev-1.13.0".to_string(),
            questions,
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "state": "Help! My payouts have been failing for 3 days.",
                "model": "jev-1.13.0",
                "questions": {
                    "is_urgent": {
                        "type": "noul",
                        "instructions": "Does this convey urgency?",
                        "criteria": { "true": "Time-sensitive", "false": "No urgency" }
                    },
                    "department": {
                        "type": "choice",
                        "instructions": "Which team should handle this?",
                        "criteria": { "billing": "Payments, invoicing, refunds", "technical": null }
                    },
                    "frustration": {
                        "type": "score",
                        "instructions": "How frustrated is the customer?",
                        "criteria": ["Calm", "Frustrated", "Very angry"]
                    },
                    "plain": { "type": "noul", "instructions": "Is it plain?" }
                }
            })
        );
    }

    #[test]
    fn response_deserializes_design_examples() {
        let response: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-1.13.0",
            "answers": {
                "a": { "type": "noul", "noul": 0.95 },
                "b": { "type": "choice", "choice": "opt_a",
                       "probabilities": { "opt_a": 0.9, "opt_b": 0.1 }, "confidence": 0.8 },
                "c": { "type": "score", "score": 1.4, "legend": { "0": "low" },
                       "probabilities": { "0": 0.1 }, "confidence": 0.7 }
            },
            "usage": { "input_tokens": 300, "output_tokens": 20 }
        }))
        .unwrap();

        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.usage.input_tokens, 300);
        assert_eq!(response.answers.noul("a"), Some(0.95));
        let choice = response.answers.choice("b").unwrap();
        assert_eq!(choice.choice, "opt_a");
        assert_eq!(choice.probabilities["opt_b"], 0.1);
        assert_eq!(choice.confidence, 0.8);
        let score = response.answers.score("c").unwrap();
        assert_eq!(score.score, 1.4);
        assert_eq!(score.confidence, 0.7);
        assert_eq!(response.answers.noul("b"), None);
        assert_eq!(response.answers.noul("missing"), None);
    }

    #[test]
    fn choice_option_limits_are_enforced() {
        assert_eq!(
            Question::choice("q", [("only", None)]).unwrap_err(),
            QuestionError::ChoiceOptions(1)
        );
        let many: Vec<(String, Option<serde_json::Value>)> =
            (0..256).map(|i| (format!("o{i}"), None)).collect();
        assert_eq!(
            Question::choice("q", many).unwrap_err(),
            QuestionError::ChoiceOptions(256)
        );
        let max: Vec<(String, Option<serde_json::Value>)> =
            (0..255).map(|i| (format!("o{i}"), None)).collect();
        assert!(Question::choice("q", max).is_ok());
    }

    #[test]
    fn score_level_limits_are_enforced() {
        assert_eq!(
            Question::score("q", ["one"]).unwrap_err(),
            QuestionError::ScoreLevels(1)
        );
        let eleven: Vec<String> = (0..11).map(|i| format!("level {i}")).collect();
        assert_eq!(
            Question::score("q", eleven).unwrap_err(),
            QuestionError::ScoreLevels(11)
        );
        assert!(Question::score("q", ["a", "b"]).is_ok());
    }

    #[test]
    fn instructions_may_be_structured_objects() {
        let question = Question::noul(json!({
            "candidate_tag": "payments",
            "question": "Does the tag `candidate_tag` describe `feature`?"
        }));
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(value["instructions"]["candidate_tag"], "payments");
    }
}
