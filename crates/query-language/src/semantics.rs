//! Three-valued logic used by runtime language and product execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    False,
    Unknown,
    True,
}
impl Truth {
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
    pub fn matches(self) -> bool {
        self == Self::True
    }
}
impl From<bool> for Truth {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}
impl std::ops::Not for Truth {
    type Output = Self;
    fn not(self) -> Self {
        Truth::not(self)
    }
}
