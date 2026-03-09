#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub String);

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id(s.to_string())
    }
}