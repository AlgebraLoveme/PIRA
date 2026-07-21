#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub text: String,
}

impl Node {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}
