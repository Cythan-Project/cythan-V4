use std::ops::Deref;

use crate::Span;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct SpannedVector<T>(pub Span, pub Vec<T>);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct SpannedObject<T>(pub Span, pub T);

impl<T> SpannedObject<T> {
    pub fn native(t: T) -> Self {
        Self(Span::default(), t)
    }
}

impl<T> Deref for SpannedObject<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.1
    }
}
impl<T> Deref for SpannedVector<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.1
    }
}
