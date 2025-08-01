use merge::Merge;

use crate::newtype_generic;

newtype_generic!(Replace, T);

impl<T> Merge for Replace<T> {
    fn merge(&mut self, other: Self) {
        replace(self, other);
    }
}

#[cfg(feature = "validator-garde")]
impl<T: garde::Validate> garde::Validate for Replace<T> {
    type Context = T::Context;

    fn validate_into(
        &self,
        ctx: &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut garde::Report,
    ) {
        self.0.validate_into(ctx, parent, report)
    }
}

pub fn replace<T>(this: &mut T, other: T) {
    *this = other;
}

#[cfg(test)]
mod tests {
    use crate::ToInner;

    use super::*;

    #[test]
    fn test_merge_replace() {
        let mut a = Replace::new(3);
        let b = Replace::new(1);

        a.merge(b);

        assert_eq!(a.to_inner(), 1);
    }
}
