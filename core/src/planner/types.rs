use serde::{Deserialize, Serialize};

/// 追问问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    pub question: String,
    pub question_type: QuestionType,
    pub options: Option<Vec<String>>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionType {
    #[serde(rename = "OpenEnded")]
    OpenEnded,
    #[serde(rename = "Choice")]
    Choice,
    #[serde(rename = "Confirmation")]
    Confirmation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clarification_question_creation() {
        let q = ClarificationQuestion {
            question: "Test?".to_string(),
            question_type: QuestionType::OpenEnded,
            options: None,
            required: true,
        };
        assert_eq!(q.question, "Test?");
    }
}
